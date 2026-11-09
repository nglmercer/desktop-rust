/*
 * Injected by wry as an initialization script (before any page script).
 *
 * Electron used to give the renderer Node integration, so it could call
 * `require('electron').ipcRenderer` directly. This shim restores that surface
 * on top of wry's `window.ipc.postMessage` + `evaluate_script` primitives.
 * No Node, no `file://`, no remote module loading.
 */
(function () {
  'use strict';

  if (window.__desktop) { return; }

  var listeners = new Map();      // channel -> Set<listener>
  var pending = new Map();        // request id -> { resolve, reject }
  var early = [];                 // events emitted before the UI subscribes
  var nextId = 1;
  var ready = false;

  function post(payload) {
    if (window.ipc && typeof window.ipc.postMessage === 'function') {
      window.ipc.postMessage(JSON.stringify(payload));
      return true;
    }
    return false;
  }

  function normalize(args) {
    return Array.isArray(args) ? args : (args === undefined ? [] : [args]);
  }

  function dispatch(channel, args) {
    args = normalize(args);
    if (channel === 'state-changed') { window.__desktop.state = args[0] || null; }
    var set = listeners.get(channel);
    if (!set || set.size === 0) {
      if (early.length < 128) { early.push([channel, args]); }
      return;
    }
    set.forEach(function (fn) {
      try { fn({ sender: null }, ...args); }
      catch (error) { console.error('[desktop] listener failed for ' + channel, error); }
    });
  }

  function resolve(id, payload) {
    var entry = pending.get(id);
    if (!entry) { return; }
    pending.delete(id);
    clearTimeout(entry.timer);
    if (payload && payload.ok) {
      entry.resolve(payload.value === undefined ? null : payload.value);
    } else {
      var detail = (payload && payload.error) || { message: 'IPC call failed' };
      var error = new Error(detail.message || 'IPC call failed');
      Object.assign(error, detail);
      entry.reject(error);
    }
  }

  function invoke(channel, ...args) {
    return new Promise(function (resolvePromise, reject) {
      var id = nextId++;
      var timer = setTimeout(function () {
        if (pending.has(id)) {
          pending.delete(id);
          reject(new Error('IPC timeout: ' + channel));
        }
      }, 180000);
      pending.set(id, { resolve: resolvePromise, reject: reject, timer: timer });

      if (!post({ __rpc: id, channel: channel, args: args })) {
        // Fallback for hosts where postMessage is not registered yet.
        fetch('/__invoke', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ channel: channel, args: args })
        })
          .then(function (response) { return response.json(); })
          .then(function (payload) { resolve(id, payload); })
          .catch(function (error) {
            pending.delete(id);
            clearTimeout(timer);
            reject(error);
          });
      }
    });
  }

  // Node's synchronous `fs` calls have no asynchronous equivalent the
  // renderer could await, so they go through a blocking request to the
  // desktop:// protocol. Only the stateless desktop.io.* services use this.
  function invokeSync(channel, ...args) {
    var request = new XMLHttpRequest();
    // The payload travels in the URL: WebKitGTK does not hand the body of a
    // custom-protocol request to the application.
    var payload = encodeURIComponent(JSON.stringify({ channel: channel, args: args }));
    // WebKit rejects URLs past roughly 64 KB with an opaque network error.
    if (payload.length > 60000) {
      throw Object.assign(new Error('synchronous IPC payload too large: ' + channel), { code: 'E2BIG' });
    }
    request.open('POST', '/__invoke?payload=' + payload, false);
    request.send();
    var reply = JSON.parse(request.responseText);
    if (reply && reply.ok) {
      return reply.value === undefined ? null : reply.value;
    }
    var detail = (reply && reply.error) || { message: 'IPC call failed' };
    throw Object.assign(new Error(detail.message || 'IPC call failed'), detail);
  }

  function send(channel, ...args) {
    if (!post({ channel: channel, args: args })) {
      console.warn('[desktop] dropping simplex message (no transport):', channel);
    }
  }

  function on(channel, listener) {
    if (!listeners.has(channel)) { listeners.set(channel, new Set()); }
    listeners.get(channel).add(listener);
    return window.__desktop;
  }

  function once(channel, listener) {
    function wrapper(event, ...args) {
      removeListener(channel, wrapper);
      listener(event, ...args);
    }
    return on(channel, wrapper);
  }

  function removeListener(channel, listener) {
    var set = listeners.get(channel);
    if (set) { set.delete(listener); }
    return window.__desktop;
  }

  function removeAllListeners(channel) {
    if (channel === undefined) { listeners.clear(); }
    else { listeners.delete(channel); }
    return window.__desktop;
  }

  function markReady() {
    if (ready) { return; }
    ready = true;
    var queued = early.splice(0);
    queued.forEach(function (entry) { dispatch(entry[0], entry[1]); });
  }

  var bridge = {
    // renderer -> Rust
    invoke: invoke,
    invokeSync: invokeSync,
    send: send,
    // Electron's three synchronous channels have no wry equivalent. They now
    // return a promise and callers must await them explicitly.
    sendSync: function (channel, ...args) {
      console.warn('[desktop] sendSync(' + channel + ') is asynchronous under wry');
      return invoke(channel, ...args);
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
    ready: markReady,
    // Last sanitized AppState snapshot, always kept fresh even before the UI
    // has registered its state-changed listener.
    state: null,
    platform: 'wry',
    runtime: 'tao + wry',
    isElectron: false
  };

  window.__desktop = bridge;
  window.ipcRenderer = bridge;
  window.desktop = {
    invoke: invoke,
    send: send,
    on: on,
    once: once,
    getState: function () { return bridge.state; },
    ready: markReady,
    platform: 'wry',
    runtime: 'tao + wry'
  };

  // Minimal compatibility surface for a browser build of the legacy React
  // renderer. TypeScript modules that reached into Node (`fs`, `path`,
  // `dugite`, `api.ts`) must use the explicit `desktop.*` Rust commands.
  var electronShim = {
    ipcRenderer: bridge,
    clipboard: {
      writeText: function (text) { return invoke('write-clipboard-text', text); }
    },
    shell: {
      openExternal: function (url) { return invoke('open-external', url); },
      showItemInFolder: function (path) { return invoke('show-item-in-folder', path); },
      moveItemToTrash: function (path) { return invoke('move-to-trash', path); }
    },
    webFrame: {
      setZoomFactor: function (factor) { send('set-window-zoom-factor', factor); }
    }
  };

  if (!window.process) {
    window.process = { platform: 'win32', type: 'renderer', env: {}, versions: {} };
  }
  if (!window.require) {
    window.require = function (name) {
      if (name === 'electron') { return electronShim; }
      throw new Error('[desktop] Node module "' + name + '" is not available under wry; use a desktop.* command');
    };
  }

  // --- fetch ---------------------------------------------------------------
  // Electron's renderer was exempt from CORS; a page served from desktop://
  // is not, and GitHub's OAuth token endpoint sends no CORS headers at all.
  // Requests to other origins are performed by Rust instead.
  function toBase64(bytes) {
    var binary = '';
    for (var i = 0; i < bytes.length; i += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(binary);
  }
  function fromBase64(text) {
    var binary = atob(text);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i++) { bytes[i] = binary.charCodeAt(i); }
    return bytes;
  }
  bridge.encodeBase64 = toBase64;
  bridge.decodeBase64 = fromBase64;

  var nativeFetch = window.fetch.bind(window);
  function abortError(signal) {
    return signal.reason !== undefined
      ? signal.reason
      : new DOMException('The operation was aborted.', 'AbortError');
  }
  window.fetch = function (input, init) {
    var request;
    try { request = new Request(input, init); }
    catch (error) { return Promise.reject(error); }

    var target = new URL(request.url, document.baseURI);
    if ((target.protocol !== 'http:' && target.protocol !== 'https:') || target.origin === location.origin) {
      return nativeFetch(input, init);
    }

    var signal = request.signal;
    if (signal && signal.aborted) { return Promise.reject(abortError(signal)); }

    var headers = [];
    request.headers.forEach(function (value, name) { headers.push([name, value]); });
    // `new Request` silently drops headers a browser forbids (User-Agent, ...).
    // The caller asked for them and Rust can send them.
    var requested = init && init.headers;
    if (requested && !(requested instanceof Headers)) {
      var pairs = Array.isArray(requested) ? requested : Object.entries(requested);
      pairs.forEach(function (pair) {
        var name = String(pair[0]).toLowerCase();
        if (!headers.some(function (existing) { return existing[0].toLowerCase() === name; })) {
          headers.push([String(pair[0]), String(pair[1])]);
        }
      });
    }

    var hasBody = request.method !== 'GET' && request.method !== 'HEAD';
    var proxied = (hasBody ? request.arrayBuffer() : Promise.resolve(null)).then(function (body) {
      return invoke('desktop.io.http-fetch', {
        url: target.href,
        method: request.method,
        headers: headers,
        redirect: request.redirect,
        body: body && body.byteLength > 0 ? toBase64(new Uint8Array(body)) : undefined
      });
    }).then(function (result) {
      var empty = result.status === 204 || result.status === 205 || result.status === 304 || request.method === 'HEAD';
      var response = new Response(empty ? null : fromBase64(result.body), {
        status: result.status,
        statusText: result.statusText,
        headers: result.headers
      });
      Object.defineProperty(response, 'url', { value: result.url });
      return response;
    }, function (error) {
      // fetch reports every network-level failure as a TypeError.
      throw new TypeError(error && error.message ? error.message : 'Failed to fetch');
    });

    if (!signal) { return proxied; }
    return new Promise(function (resolve, reject) {
      var onAbort = function () { reject(abortError(signal)); };
      signal.addEventListener('abort', onAbort, { once: true });
      proxied.then(resolve, reject).finally(function () {
        signal.removeEventListener('abort', onAbort);
      });
    });
  };

  // The renderer was written for Chromium. WebKit (WKWebView / WebKitGTK)
  // has no requestIdleCallback.
  if (typeof window.requestIdleCallback !== 'function') {
    window.requestIdleCallback = function (callback, options) {
      var start = Date.now();
      return setTimeout(function () {
        callback({
          didTimeout: false,
          timeRemaining: function () { return Math.max(0, 50 - (Date.now() - start)); }
        });
      }, options && options.timeout ? Math.min(options.timeout, 1) : 1);
    };
    window.cancelIdleCallback = function (id) { clearTimeout(id); };
  }

  // Electron showed renderer failures in DevTools and the main-process log.
  // A wry window has neither by default, so a failed boot is a silent blank
  // page. Forward uncaught errors to the Rust log.
  function report(kind, detail) {
    try { post({ channel: 'log', args: ['error', '[renderer ' + kind + '] ' + detail] }); }
    catch (_) { /* never let reporting throw */ }
  }
  function describe(value) {
    if (value instanceof Error || (value && value.stack)) {
      return (value.name || 'Error') + ': ' + String(value.message || value) + '\n' + (value.stack || '');
    }
    if (typeof value === 'string') { return value; }
    var text;
    try { text = JSON.stringify(value); } catch (_) { text = undefined; }
    // An object with no enumerable fields says nothing as JSON.
    if (text === undefined || text === '{}') {
      var kind = value && value.constructor ? value.constructor.name : typeof value;
      return '[' + kind + '] ' + String(value);
    }
    return text;
  }
  window.addEventListener('error', function (event) {
    if (event.target && event.target !== window && (event.target.src || event.target.href)) {
      report('resource', 'failed to load ' + (event.target.src || event.target.href));
      return;
    }
    report('error', (event.message || 'unknown') + ' at ' + event.filename + ':' + event.lineno + ':' + event.colno +
      (event.error && event.error.stack ? '\n' + event.error.stack : ''));
  }, true);
  window.addEventListener('unhandledrejection', function (event) {
    report('unhandledrejection', describe(event.reason));
  });
  var consoleError = console.error.bind(console);
  console.error = function () {
    consoleError.apply(null, arguments);
    report('console.error', Array.prototype.map.call(arguments, describe).join(' '));
  };

  // Safety net: if the UI never calls ready(), flush queued events anyway.
  window.addEventListener('DOMContentLoaded', function () {
    setTimeout(markReady, 0);
  }, { once: true });
})();
