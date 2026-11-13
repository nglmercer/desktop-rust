export const trampolineEnvironment = {}
export const trampolineServer = { start: async () => {}, close: async () => {} }
export function createTrampolineHandler() {
  return async (_r: any) => {}
}
export function getDesktopAskpassTrampolineFilename() {
  return 'desktop-askpass'
}
export function getDesktopCredentialHelperTrampolineFilename() {
  return 'desktop-credential-helper'
}
export function getDesktopTrampolineFilename() {
  return 'desktop-trampoline'
}
export const getTrampolinePath = () => ''
