/*
 * Injected by wry as an initialization script (before any page script).
 *
 * Electron used to give the renderer Node integration, so it could call
 * `require('electron').ipcRenderer` directly. This shim restores that surface
 * on top of wry's `window.ipc.postMessage` + `evaluate_script` primitives.
 * No Node, no `file://`, no remote module loading.
 */
type IpcListener = (event: { sender: null }, ...args: unknown[]) => void
type IpcProgress = (payload: unknown) => void
type IpcReply = {
  ok?: boolean
  value?: unknown
  error?: { message?: string; [key: string]: unknown }
}
type Pending = {
  resolve: (value: unknown) => void
  reject: (reason: unknown) => void
  timer: number
  progress: IpcProgress | null
}
type MenuItem = {
  role?: string
  visible?: boolean
  type?: string
  label?: string
  enabled?: boolean
  checked?: boolean
  submenu?: MenuItem[]
}
type Bridge = {
  [key: string]: unknown
  state: unknown
  on: (channel: string, listener: IpcListener) => Bridge | undefined
  removeListener: (channel: string, listener: IpcListener) => Bridge | undefined
}
type NativeWindow = Window & {
  ipc?: { postMessage: (message: string) => void }
  __desktop?: Bridge
  ipcRenderer?: unknown
  desktop?: unknown
  process?: unknown
  require?: (name: string) => unknown
}

;(function () {
  'use strict'
  const nativeWindow = window as NativeWindow

  if (nativeWindow.__desktop) {
    return
  }

  var listeners = new Map<string, Set<IpcListener>>() // channel -> Set<listener>
  var pending = new Map<number, Pending>() // request id -> { resolve, reject }
  var early: Array<[string, unknown[]]> = [] // events emitted before the UI subscribes
  var nextId = 1
  var ready = false

  function post(payload: unknown) {
    if (
      nativeWindow.ipc &&
      typeof nativeWindow.ipc.postMessage === 'function'
    ) {
      nativeWindow.ipc.postMessage(JSON.stringify(payload))
      return true
    }
    return false
  }

  function normalize(args: unknown): unknown[] {
    return Array.isArray(args) ? args : args === undefined ? [] : [args]
  }

  function dispatch(channel: string, args: unknown) {
    const normalizedArgs = normalize(args)
    if (channel === 'state-changed') {
      nativeWindow.__desktop!.state = normalizedArgs[0] || null
    }
    var set = listeners.get(channel)
    if (!set || set.size === 0) {
      if (early.length < 128) {
        early.push([channel, normalizedArgs])
      }
      return
    }
    set.forEach(function (fn) {
      try {
        fn({ sender: null }, ...normalizedArgs)
      } catch (error) {
        console.error('[desktop] listener failed for ' + channel, error)
      }
    })
  }

  function resolve(id: number, payload: IpcReply | null) {
    var entry = pending.get(id)
    if (!entry) {
      return
    }
    pending.delete(id)
    clearTimeout(entry.timer)
    if (payload && payload.ok) {
      entry.resolve(payload.value === undefined ? null : payload.value)
    } else {
      var detail = (payload && payload.error) || { message: 'IPC call failed' }
      var error = new Error(detail.message || 'IPC call failed')
      Object.assign(error, detail)
      entry.reject(error)
    }
  }

  // Progress is scoped to the pending invocation rather than broadcast to
  // channel listeners. Late events after completion are discarded.
  function progress(id: number, payload: unknown) {
    var entry = pending.get(id)
    if (entry && entry.progress) {
      try {
        entry.progress(payload)
      } catch (error) {
        console.error('[desktop] progress listener failed', error)
      }
    }
  }

  // Normal IPC promises the renderer may keep open. Past this, a request is
  // refused at once rather than queued without bound (Rust applies the same
  // kind of limit on its side; see `resources` in __health, published by
  // `desktop.get-app-info`. Mutated once after the first health read so the
  // low-memory profile (64) also reaches the frontend without a rebuild.
  var PENDING_LIMIT = 128
  var healthRead = false
  function refreshPendingLimit() {
    if (healthRead) {
      return
    }
    healthRead = true
    try {
      var xhr = new XMLHttpRequest()
      xhr.open('GET', 'desktop://localhost/__health', true)
      xhr.onload = function () {
        try {
          var payload = JSON.parse(xhr.responseText)
          // `__health` nests under invoke result; `get-app-info` below carries the same.
          var pending = payload?.pendingPromises ?? payload?.pending_promises
          if (typeof pending === 'number' && pending > 0) {
            PENDING_LIMIT = pending
            bridge.pendingPromises = pending
          }
        } catch (e) {}
      }
      xhr.send()
    } catch (e) {}
  }
  setTimeout(refreshPendingLimit, 0)

  function invoke(channel: string, ...args: unknown[]) {
    return invokeRequest(channel, null, args)
  }

  function invokeWithProgress(
    channel: string,
    onProgress: IpcProgress,
    ...args: unknown[]
  ) {
    return invokeRequest(channel, onProgress, args)
  }

  function invokeRequest(
    channel: string,
    onProgress: IpcProgress | null,
    args: unknown[]
  ) {
    // Electron drew a native popup for this request. The page draws the same
    // menu instead; see showContextMenu below.
    if (channel === 'show-contextual-menu') {
      return showContextMenu((args[0] || []) as MenuItem[])
    }
    if (pending.size >= PENDING_LIMIT) {
      return Promise.reject(
        Object.assign(
          new Error(
            'EBUSY: QUEUE_FULL, ' +
              pending.size +
              ' IPC calls are already waiting'
          ),
          { code: 'EBUSY' }
        )
      )
    }
    return new Promise(function (resolvePromise, reject) {
      var id = nextId++
      var timeout = channel === 'desktop.backend.copilot' ? 360000 : 180000
      var request = args[0] as
        | {
            operation?: { type?: string; operation?: { method?: string } }
            deadline_ms?: number
          }
        | undefined
      var operation = request && request.operation
      if (
        channel === 'desktop.backend.git_info' &&
        operation &&
        ((operation.type === 'fetch_remote' &&
          operation.operation &&
          (operation.operation.method === 'renderer_fetch' ||
            operation.operation.method === 'renderer_remote_head' ||
            operation.operation.method === 'renderer_push')) ||
          operation.type === 'clone_selected' ||
          operation.type === 'recover_clone') &&
        request &&
        typeof request.deadline_ms === 'number' &&
        Number.isInteger(request.deadline_ms) &&
        request.deadline_ms >= 1 &&
        request.deadline_ms <= 3600000
      ) {
        timeout = Math.max(timeout, request.deadline_ms + 30000)
      }
      var timer = setTimeout(function () {
        if (pending.has(id)) {
          pending.delete(id)
          reject(new Error('IPC timeout: ' + channel))
        }
      }, timeout)
      pending.set(id, {
        resolve: resolvePromise,
        reject: reject,
        timer: timer,
        progress: onProgress,
      })

      var posted: boolean
      try {
        posted = post({
          __rpc: id,
          __progress: !!onProgress,
          channel: channel,
          args: args,
        })
      } catch {
        pending.delete(id)
        clearTimeout(timer)
        reject(Object.assign(new Error('Native IPC submission could not be confirmed'), { code: 'disconnected' }))
        return
      }
      if (!posted) {
        // The HTTP fallback cannot deliver request-owned progress. Refuse
        // before submitting the operation instead of silently dropping it.
        if (onProgress || (channel === 'desktop.backend.git_info' && operation &&
          (operation.type === 'clone_selected' || operation.type === 'recover_clone'))) {
          pending.delete(id)
          clearTimeout(timer)
          reject(
            Object.assign(
              new Error('This operation requires native IPC'),
              { code: 'unsupported' }
            )
          )
          return
        }
        // Fallback for hosts where postMessage is not registered yet.
        fetch('/__invoke', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ channel: channel, args: args }),
        })
          .then(function (response) {
            return response.json()
          })
          .then(function (payload) {
            resolve(id, payload)
          })
          .catch(function (error) {
            pending.delete(id)
            clearTimeout(timer)
            reject(error)
          })
      }
    })
  }

  // Node's synchronous `fs` calls have no asynchronous equivalent the
  // renderer could await, so they go through a blocking request to the
  // desktop:// protocol. Only the stateless desktop.io.* services use this.
  function invokeSync(channel: string, ...args: unknown[]) {
    var request = new XMLHttpRequest()
    // The payload travels in the URL: WebKitGTK does not hand the body of a
    // custom-protocol request to the application.
    var payload = encodeURIComponent(
      JSON.stringify({ channel: channel, args: args })
    )
    // WebKit rejects URLs past roughly 64 KB with an opaque network error.
    if (payload.length > 60000) {
      throw Object.assign(
        new Error('synchronous IPC payload too large: ' + channel),
        { code: 'E2BIG' }
      )
    }
    // `sync=1` selects Rust's reserved synchronous lane, which accepts only
    // short host IO. Long work is refused there, not queued behind others.
    request.open('POST', '/__invoke?sync=1&payload=' + payload, false)
    request.send()
    var reply = JSON.parse(request.responseText)
    if (reply && reply.ok) {
      return reply.value === undefined ? null : reply.value
    }
    var detail = (reply && reply.error) || { message: 'IPC call failed' }
    throw Object.assign(new Error(detail.message || 'IPC call failed'), detail)
  }

  function send(channel: string, ...args: unknown[]) {
    if (!post({ channel: channel, args: args })) {
      if (channel === 'desktop.backend.cancel') {
        throw new Error('Cancellation transport is unavailable')
      }
      console.warn(
        '[desktop] dropping simplex message (no transport):',
        channel
      )
    }
  }

  function on(channel: string, listener: IpcListener) {
    if (!listeners.has(channel)) {
      listeners.set(channel, new Set())
    }
    listeners.get(channel)!.add(listener)
    return nativeWindow.__desktop
  }

  function once(channel: string, listener: IpcListener) {
    function wrapper(event: { sender: null }, ...args: unknown[]) {
      removeListener(channel, wrapper)
      listener(event, ...args)
    }
    return on(channel, wrapper)
  }

  function removeListener(channel: string, listener: IpcListener) {
    var set = listeners.get(channel)
    if (set) {
      set.delete(listener)
    }
    return nativeWindow.__desktop
  }

  function removeAllListeners(channel?: string) {
    if (channel === undefined) {
      listeners.clear()
    } else {
      listeners.delete(channel)
    }
    return nativeWindow.__desktop
  }

  function markReady() {
    if (ready) {
      return
    }
    ready = true
    var queued = early.splice(0)
    queued.forEach(function (entry) {
      dispatch(entry[0], entry[1])
    })
  }

  var bridge: Bridge = {
    // renderer -> Rust
    invoke: invoke,
    invokeWithProgress: invokeWithProgress,
    supportsCommitRequestProgress: true,
    invokeSync: invokeSync,
    send: send,
    supportsCancellation: function () {
      return !!(
        nativeWindow.ipc && typeof nativeWindow.ipc.postMessage === 'function'
      )
    },
    pendingPromises: PENDING_LIMIT,
    // Electron's three synchronous channels have no wry equivalent. They now
    // return a promise and callers must await them explicitly.
    sendSync: function (channel: string, ...args: unknown[]) {
      console.warn(
        '[desktop] sendSync(' + channel + ') is asynchronous under wry'
      )
      return invoke(channel, ...args)
    },
    // Rust -> renderer
    on: on,
    once: once,
    removeListener: removeListener,
    removeAllListeners: removeAllListeners,
    addListener: on,
    off: removeListener,
    // Called by Rust through evaluate_script.
    emit: dispatch,
    resolve: resolve,
    progress: progress,
    ready: markReady,
    // Last sanitized AppState snapshot, always kept fresh even before the UI
    // has registered its state-changed listener.
    state: null,
    platform: 'wry',
    runtime: 'tao + wry',
    isElectron: false,
  }

  nativeWindow.__desktop = bridge
  nativeWindow.ipcRenderer = bridge
  nativeWindow.desktop = {
    invoke: invoke,
    send: send,
    on: on,
    once: once,
    getState: function () {
      return bridge.state
    },
    ready: markReady,
    platform: 'wry',
    runtime: 'tao + wry',
  }

  // Minimal compatibility surface for a browser build of the legacy React
  // renderer. TypeScript modules that reached into Node (`fs`, `path`,
  // `dugite`, `api.ts`) must use the explicit `desktop.*` Rust commands.
  var electronShim = {
    ipcRenderer: bridge,
    clipboard: {
      writeText: function (text: string) {
        return invoke('write-clipboard-text', text)
      },
    },
    shell: {
      openExternal: function (url: string) {
        return invoke('open-external', url)
      },
      showItemInFolder: function (path: string) {
        return invoke('show-item-in-folder', path)
      },
      moveItemToTrash: function (path: string) {
        return invoke('move-to-trash', path)
      },
    },
    webFrame: {
      setZoomFactor: function (factor: number) {
        send('set-window-zoom-factor', factor)
      },
    },
  }

  if (!nativeWindow.process) {
    nativeWindow.process = {
      platform: 'win32',
      type: 'renderer',
      env: {},
      versions: {},
    }
  }
  if (!nativeWindow.require) {
    nativeWindow.require = function (name: string) {
      if (name === 'electron') {
        return electronShim
      }
      throw new Error(
        '[desktop] Node module "' +
          name +
          '" is not available under wry; use a desktop.* command'
      )
    }
  }

  // --- context menu -----------------------------------------------------------
  // Electron resolved `show-contextual-menu` with the index path of the clicked
  // item (for example [2, 0]), or null when the menu was dismissed. The same
  // contract is served here, drawn in the page, so the renderer's callers are
  // unchanged.
  var lastPointer = { x: 0, y: 0 }
  function rememberPointer(event: MouseEvent) {
    if (typeof event.clientX === 'number') {
      lastPointer = { x: event.clientX, y: event.clientY }
    }
  }
  document.addEventListener('contextmenu', rememberPointer, true)
  document.addEventListener('mousedown', rememberPointer, true)

  // The Edit menu Electron builds for `{ role: 'editMenu' }`.
  var editMenuRows: Array<{
    label?: string
    command?: string
    separator?: boolean
  }> = [
    { label: 'Undo', command: 'undo' },
    { label: 'Redo', command: 'redo' },
    { separator: true },
    { label: 'Cut', command: 'cut' },
    { label: 'Copy', command: 'copy' },
    { label: 'Paste', command: 'paste' },
    { separator: true },
    { label: 'Select All', command: 'selectAll' },
  ]

  var contextMenuStyle = [
    '.gd-ctx{position:fixed;z-index:10000;min-width:180px;padding:4px 0;border-radius:6px;',
    'font:13px/1.4 "Noto Sans",system-ui,sans-serif;background:#ffffff;color:#1f2328;',
    'border:1px solid #d0d7de;box-shadow:0 8px 24px rgba(31,35,40,.2);user-select:none}',
    '.gd-ctx .gd-row{display:flex;align-items:center;gap:8px;padding:5px 12px;cursor:default;white-space:nowrap}',
    '.gd-ctx .gd-row.enabled:hover,.gd-ctx .gd-row.enabled.open{background:#0969da;color:#ffffff}',
    '.gd-ctx .gd-row.disabled{color:#8c959f}',
    '.gd-ctx .gd-check{width:12px;text-align:center}',
    '.gd-ctx .gd-arrow{margin-left:auto;padding-left:16px}',
    '.gd-ctx .gd-sep{height:1px;margin:4px 0;background:#d0d7de}',
    '.gd-ctx .gd-sub{position:absolute;display:none}',
    '.gd-ctx .gd-row.open>.gd-sub{display:block}',
    'body.theme-dark .gd-ctx{background:#2d333b;color:#adbac7;border-color:#444c56;box-shadow:0 8px 24px rgba(0,0,0,.5)}',
    'body.theme-dark .gd-ctx .gd-row.enabled:hover,body.theme-dark .gd-ctx .gd-row.enabled.open{background:#316dca;color:#ffffff}',
    'body.theme-dark .gd-ctx .gd-row.disabled{color:#636e7b}',
    'body.theme-dark .gd-ctx .gd-sep{background:#444c56}',
  ].join('')

  function ensureContextMenuStyle() {
    if (document.getElementById('gd-ctx-style')) {
      return
    }
    var style = document.createElement('style')
    style.id = 'gd-ctx-style'
    style.textContent = contextMenuStyle
    document.head.appendChild(style)
  }

  function runEditCommand(command: string) {
    if (command === 'selectAll') {
      document.execCommand('selectAll')
      return
    }
    document.execCommand(command)
  }

  // Spell-check suggestions (addSpellCheckMenu) are not available in the webview.
  function showContextMenu(items: MenuItem[]) {
    ensureContextMenuStyle()
    return new Promise(function (resolvePromise) {
      var done = false
      var root = document.createElement('div')
      root.className = 'gd-ctx'
      root.setAttribute('role', 'menu')

      function close(result: number[] | null) {
        if (done) {
          return
        }
        done = true
        document.removeEventListener('mousedown', onOutside, true)
        document.removeEventListener('keydown', onKey, true)
        window.removeEventListener('blur', onDismiss)
        window.removeEventListener('resize', onDismiss)
        root.remove()
        resolvePromise(result)
      }
      function onDismiss() {
        close(null)
      }
      function onOutside(event: MouseEvent) {
        if (!root.contains(event.target as Node | null)) {
          close(null)
        }
      }
      function onKey(event: KeyboardEvent) {
        if (event.key === 'Escape') {
          event.preventDefault()
          close(null)
        }
      }

      function row(
        label: string,
        options: { enabled: boolean; checked: boolean }
      ) {
        var element = document.createElement('div')
        element.className =
          'gd-row ' + (options.enabled ? 'enabled' : 'disabled')
        element.setAttribute('role', 'menuitem')
        var check = document.createElement('span')
        check.className = 'gd-check'
        check.textContent = options.checked ? '✓' : ''
        element.appendChild(check)
        var text = document.createElement('span')
        text.textContent = label
        element.appendChild(text)
        return element
      }

      function separator() {
        var element = document.createElement('div')
        element.className = 'gd-sep'
        element.setAttribute('role', 'separator')
        return element
      }

      function panel(list: MenuItem[], indices: number[]) {
        var container = document.createElement('div')
        container.className = 'gd-sub gd-ctx'
        container.style.position = 'absolute'
        container.style.left = '100%'
        container.style.top = '-5px'
        container.style.boxShadow = 'none'
        appendItems(container, list, indices)
        return container
      }

      function appendItems(
        container: HTMLElement,
        list: MenuItem[],
        indices: number[]
      ) {
        list.forEach(function (item, index) {
          if (!item) {
            return
          }
          if (item.role && String(item.role).toLowerCase() === 'editmenu') {
            editMenuRows.forEach(function (edit) {
              if (edit.separator) {
                container.appendChild(separator())
                return
              }
              var editRow = row(edit.label ?? '', {
                enabled: true,
                checked: false,
              })
              editRow.addEventListener('click', function () {
                runEditCommand(edit.command ?? '')
                close(null)
              })
              container.appendChild(editRow)
            })
            return
          }
          if (item.visible === false) {
            return
          }
          if (item.type === 'separator') {
            container.appendChild(separator())
            return
          }

          var enabled = item.enabled !== false
          var element = row(item.label || '', {
            enabled: enabled,
            checked: item.type === 'checkbox' && item.checked === true,
          })
          var itemIndices = indices.concat([index])

          if (Array.isArray(item.submenu) && item.submenu.length > 0) {
            element.classList.add('has-submenu')
            var arrow = document.createElement('span')
            arrow.className = 'gd-arrow'
            arrow.textContent = '▸'
            element.appendChild(arrow)
            element.style.position = 'relative'
            element.appendChild(panel(item.submenu, itemIndices))
            element.addEventListener('mouseenter', function () {
              if (enabled) {
                element.classList.add('open')
              }
            })
            element.addEventListener('mouseleave', function () {
              element.classList.remove('open')
            })
            container.appendChild(element)
            return
          }

          if (enabled) {
            element.addEventListener('click', function (event) {
              event.stopPropagation()
              close(itemIndices)
            })
          }
          container.appendChild(element)
        })
      }

      appendItems(root, Array.isArray(items) ? items : [], [])

      document.body.appendChild(root)

      // Place the menu at the pointer, then keep it inside the viewport.
      var rect = root.getBoundingClientRect()
      var x = Math.min(lastPointer.x, window.innerWidth - rect.width - 4)
      var y = Math.min(lastPointer.y, window.innerHeight - rect.height - 4)
      root.style.left = Math.max(4, x) + 'px'
      root.style.top = Math.max(4, y) + 'px'

      // Opening a submenu on the right needs room; flip it left at the edge.
      Array.prototype.forEach.call(
        root.querySelectorAll('.gd-sub'),
        function (sub) {
          var box = sub.parentElement.getBoundingClientRect()
          if (box.right + sub.offsetWidth > window.innerWidth) {
            sub.style.left = 'auto'
            sub.style.right = '100%'
          }
        }
      )

      // Mouse-up and key handling start after this event finishes, so the click
      // that opened the menu cannot close it.
      setTimeout(function () {
        document.addEventListener('mousedown', onOutside, true)
        document.addEventListener('keydown', onKey, true)
        window.addEventListener('blur', onDismiss)
        window.addEventListener('resize', onDismiss)
      }, 0)
    })
  }

  // --- fetch ---------------------------------------------------------------
  // Electron's renderer was exempt from CORS; a page served from desktop://
  // is not, and GitHub's OAuth token endpoint sends no CORS headers at all.
  // Requests to other origins are performed by Rust instead.
  function toBase64(bytes: Uint8Array) {
    var binary = ''
    for (var i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode.apply(
        null,
        Array.from(bytes.subarray(i, i + 0x8000))
      )
    }
    return btoa(binary)
  }
  function fromBase64(text: string) {
    var binary = atob(text)
    var bytes = new Uint8Array(binary.length)
    for (var i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i)
    }
    return bytes
  }
  bridge.encodeBase64 = toBase64
  bridge.decodeBase64 = fromBase64

  var nativeFetch = window.fetch.bind(window)
  function abortError(signal: AbortSignal) {
    return signal.reason !== undefined
      ? signal.reason
      : new DOMException('The operation was aborted.', 'AbortError')
  }
  window.fetch = function (input, init) {
    var request: Request
    try {
      request = new Request(input, init)
    } catch (error) {
      return Promise.reject(error)
    }

    var target = new URL(request.url, document.baseURI)
    if (
      (target.protocol !== 'http:' && target.protocol !== 'https:') ||
      target.origin === location.origin
    ) {
      return nativeFetch(input, init)
    }

    var signal = request.signal
    if (signal && signal.aborted) {
      return Promise.reject(abortError(signal))
    }

    var headers: Array<[string, string]> = []
    request.headers.forEach(function (value, name) {
      headers.push([name, value])
    })
    // `new Request` silently drops headers a browser forbids (User-Agent, ...).
    // The caller asked for them and Rust can send them.
    var requested = init && init.headers
    if (requested && !(requested instanceof Headers)) {
      var pairs = Array.isArray(requested)
        ? requested
        : Object.entries(requested)
      pairs.forEach(function (pair) {
        var name = String(pair[0]).toLowerCase()
        if (
          !headers.some(function (existing) {
            return existing[0].toLowerCase() === name
          })
        ) {
          headers.push([String(pair[0]), String(pair[1])])
        }
      })
    }

    var hasBody = request.method !== 'GET' && request.method !== 'HEAD'
    var proxied = (hasBody ? request.arrayBuffer() : Promise.resolve(null))
      .then(function (body) {
        return invoke('desktop.io.http-fetch', {
          url: target.href,
          method: request.method,
          headers: headers,
          redirect: request.redirect,
          body:
            body && body.byteLength > 0
              ? toBase64(new Uint8Array(body))
              : undefined,
        })
      })
      .then(
        function (value) {
          const result = value as {
            status: number
            statusText: string
            headers: HeadersInit
            body: string
            url: string
          }
          var empty =
            result.status === 204 ||
            result.status === 205 ||
            result.status === 304 ||
            request.method === 'HEAD'
          var response = new Response(empty ? null : fromBase64(result.body), {
            status: result.status,
            statusText: result.statusText,
            headers: result.headers,
          })
          Object.defineProperty(response, 'url', { value: result.url })
          return response
        },
        function (error) {
          // fetch reports every network-level failure as a TypeError.
          throw new TypeError(
            error && error.message ? error.message : 'Failed to fetch'
          )
        }
      )

    if (!signal) {
      return proxied
    }
    return new Promise(function (resolve, reject) {
      var onAbort = function () {
        reject(abortError(signal))
      }
      signal.addEventListener('abort', onAbort, { once: true })
      proxied.then(resolve, reject).finally(function () {
        signal.removeEventListener('abort', onAbort)
      })
    })
  }

  // The renderer was written for Chromium. WebKit (WKWebView / WebKitGTK)
  // has no requestIdleCallback.
  if (typeof window.requestIdleCallback !== 'function') {
    window.requestIdleCallback = function (callback, options) {
      var start = Date.now()
      return setTimeout(
        function () {
          callback({
            didTimeout: false,
            timeRemaining: function () {
              return Math.max(0, 50 - (Date.now() - start))
            },
          })
        },
        options && options.timeout ? Math.min(options.timeout, 1) : 1
      )
    }
    window.cancelIdleCallback = function (id) {
      clearTimeout(id)
    }
  }

  // Electron showed renderer failures in DevTools and the main-process log.
  // A wry window has neither by default, so a failed boot is a silent blank
  // page. Forward uncaught errors to the Rust log.
  function report(kind: string, detail: string) {
    try {
      post({
        channel: 'log',
        args: ['error', '[renderer ' + kind + '] ' + detail],
      })
    } catch (_) {
      /* never let reporting throw */
    }
  }
  function describe(value: unknown) {
    const detail = value as
      | {
          name?: unknown
          message?: unknown
          stack?: unknown
          constructor?: { name?: string }
        }
      | null
      | undefined
    if (value instanceof Error || (value && detail?.stack)) {
      return (
        (detail?.name || 'Error') +
        ': ' +
        String(detail?.message || value) +
        '\n' +
        (detail?.stack || '')
      )
    }
    if (typeof value === 'string') {
      return value
    }
    var text
    try {
      text = JSON.stringify(value)
    } catch (_) {
      text = undefined
    }
    // An object with no enumerable fields says nothing as JSON.
    if (text === undefined || text === '{}') {
      var kind =
        value && detail?.constructor ? detail.constructor.name : typeof value
      return '[' + kind + '] ' + String(value)
    }
    return text
  }
  window.addEventListener(
    'error',
    function (event) {
      const target = event.target as
        | (EventTarget & { src?: string; href?: string })
        | null
      if (target && target !== window && (target.src || target.href)) {
        report('resource', 'failed to load ' + (target.src || target.href))
        return
      }
      report(
        'error',
        (event.message || 'unknown') +
          ' at ' +
          event.filename +
          ':' +
          event.lineno +
          ':' +
          event.colno +
          (event.error && event.error.stack ? '\n' + event.error.stack : '')
      )
    },
    true
  )
  window.addEventListener('unhandledrejection', function (event) {
    report('unhandledrejection', describe(event.reason))
  })
  var consoleError = console.error.bind(console)
  console.error = function () {
    consoleError(...Array.from(arguments))
    report(
      'console.error',
      Array.prototype.map.call(arguments, describe).join(' ')
    )
  }

  // Safety net: if the UI never calls ready(), flush queued events anyway.
  window.addEventListener(
    'DOMContentLoaded',
    function () {
      setTimeout(markReady, 0)
    },
    { once: true }
  )
})()
