// Used only by --smoke-test; the normal host exposes no native IPC commands.
;(() => {
  // The initialization script also runs for the initial about:blank document
  // before wry navigates to the session URL. Ignore it until we are on http.
  if (location.protocol !== 'http:') return
  const token = new URLSearchParams(location.hash.slice(1)).get('token')
  if (!token) return
  window.addEventListener(
    'DOMContentLoaded',
    async () => {
      let bridge
      try {
        // Resolve against the live URL so '/transport.js' works even when the
        // script was injected before navigation and the base is about:blank.
        const { connect } = await import(new URL('/transport.js', location.href).href)
        await import(new URL('/panels.js', location.href).href)
        bridge = connect(token)
        await bridge.ready
        const preferences = await bridge.request('get_preferences')
        if (!preferences.theme) throw new Error('Missing preferences')
        await bridge.request('list_repositories')
        const deadline = Date.now() + 10000
        while (!document.querySelector('#management-root button')) {
          if (Date.now() > deadline) throw new Error('UI did not mount')
          await new Promise(resolve => setTimeout(resolve, 25))
        }
        window.ipc.postMessage('desktop-smoke:ok')
      } catch {
        window.ipc.postMessage('desktop-smoke:error')
      } finally {
        bridge?.close()
      }
    },
    { once: true }
  )
})()
