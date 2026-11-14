/** Keep handling repeated termination signals until the supervisor has drained. */
export function installDevelopmentStopSignals(stop: () => void): () => void {
  let requested = false
  const requestStop = () => {
    if (requested) {
      return
    }
    requested = true
    stop()
  }
  const signals: NodeJS.Signals[] = ['SIGINT', 'SIGTERM']
  if (process.platform !== 'win32') {
    signals.push('SIGHUP')
  }
  for (const signal of signals) {
    process.on(signal, requestStop)
  }
  return () => {
    for (const signal of signals) {
      process.removeListener(signal, requestStop)
    }
  }
}
