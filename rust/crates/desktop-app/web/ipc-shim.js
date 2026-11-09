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
    if (value && value.stack) { return String(value.message || value) + '\n' + value.stack; }
    try { return typeof value === 'string' ? value : JSON.stringify(value); }
    catch (_) { return String(value); }
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
