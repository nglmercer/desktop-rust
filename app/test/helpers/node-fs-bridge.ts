import {
  installNodeHostBridge,
  uninstallNodeHostBridge,
} from './node-host-bridge'

/** Back-compatible name for filesystem-only consumers. */
export const installNodeFileBridge = installNodeHostBridge
export const uninstallNodeFileBridge = uninstallNodeHostBridge
