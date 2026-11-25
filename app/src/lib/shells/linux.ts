import { parseEnumValue } from '../enum'

export enum Shell {
  Gnome = 'GNOME Terminal',
  GnomeConsole = 'GNOME Console',
  Ptyxis = 'Ptyxis',
  Mate = 'MATE Terminal',
  Tilix = 'Tilix',
  Terminator = 'Terminator',
  Urxvt = 'URxvt',
  Konsole = 'Konsole',
  Xterm = 'XTerm',
  Terminology = 'Terminology',
  Deepin = 'Deepin Terminal',
  Elementary = 'Elementary Terminal',
  XFCE = 'XFCE Terminal',
  Alacritty = 'Alacritty',
  Kitty = 'Kitty',
  LXTerminal = 'LXDE Terminal',
  Warp = 'Warp',
  Ghostty = 'Ghostty',
}

export const Default = Shell.Gnome

export function parse(label: string): Shell {
  return parseEnumValue(Shell, label) ?? Default
}
