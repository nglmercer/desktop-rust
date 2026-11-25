import { parseCommandLineArgv } from 'windows-argv-parser'
import stringArgv from 'string-argv'
import { spawn, type ISpawnOptions } from '../web/child_process'
import { backend } from '../backend/client'
import type { IChildProcess } from './process/child-process'

/**
 * File extensions that can be invoked directly by `spawn` on Windows. Other
 * common Windows launcher types (e.g. `.bat`, `.cmd`, `.ps1`) require a shell
 * to execute and are intentionally excluded.
 */
export const WindowsExecutableExtensions: ReadonlyArray<string> = ['exe', 'com']

/** The string that will be replaced by the target path in the custom integration arguments */
export const TargetPathArgument = '%TARGET_PATH%'

/** The interface representing a custom integration (external editor or shell) */
export interface ICustomIntegration {
  /** The path to the custom integration */
  readonly path: string
  /** The arguments to pass to the custom integration */
  readonly arguments: string
  /** The bundle ID of the custom integration (macOS only) */
  readonly bundleID?: string
}

/**
 * Parse the arguments string of a custom integration into an array of strings.
 *
 * @param args The arguments string to parse
 */
export function parseCustomIntegrationArguments(
  args: string
): ReadonlyArray<string> {
  return __WIN32__ ? parseCommandLineArgv(args) : stringArgv(args)
}

/** Parse Windows integration arguments on the native host. */
export async function parseCustomIntegrationArgumentsForRuntime(
  args: string
): Promise<ReadonlyArray<string>> {
  if (__WIN32__) {
    return backend().parseWindowsIntegrationArguments(args)
  }
  return parseCustomIntegrationArguments(args)
}

/** Inspect a live input synchronously; the Windows host performs full parsing before save. */
export function parseCustomIntegrationArgumentsForInput(
  args: string
): ReadonlyArray<string> {
  if (__WIN32__) {
    if (args.includes('\0') || new TextEncoder().encode(args).length > 65536) {
      throw new Error('Invalid integration arguments')
    }
    // CommandLineToArgvW does not alter this placeholder. Avoid a race where
    // an asynchronous parse would leave the previous value in Preferences.
    return [args]
  }
  return parseCustomIntegrationArguments(args)
}

/**
 * Replace the target path placeholder in the custom integration arguments with
 * the actual target path.
 *
 * @param args The custom integration arguments
 * @param repoPath The target path to replace the placeholder with
 */
export function expandTargetPathArgument(
  args: ReadonlyArray<string>,
  repoPath: string
): ReadonlyArray<string> {
  // Only strip quotes when the entire argument is the quoted placeholder.
  // Otherwise preserve any user-provided quoting and replace the placeholder
  // in place.
  return args.map(arg => {
    if (
      arg === `'${TargetPathArgument}'` ||
      arg === `"${TargetPathArgument}"`
    ) {
      return repoPath
    }

    return arg.replaceAll(TargetPathArgument, repoPath)
  })
}

/**
 * Check if the custom integration arguments contain the target path placeholder.
 *
 * @param args The custom integration arguments
 */
export function checkTargetPathArgument(args: ReadonlyArray<string>): boolean {
  return args.some(arg => arg.includes(TargetPathArgument))
}

/**
 * Validate the path of a custom integration.
 *
 * @param path The path to the custom integration
 *
 * @returns An object with a boolean indicating if the path is valid and, if
 *          the path is a macOS app, the bundle ID of the app.
 */
export async function validateCustomIntegrationPath(
  path: string
): Promise<{ isValid: boolean; bundleID?: string }> {
  if (path.length === 0) {
    return { isValid: false }
  }

  try {
    return await backend().validateCustomIntegrationPath(path)
  } catch (e) {
    if (e.code !== 'ENOENT') {
      log.error(`Failed to validate path: ${path}`, e)
    }
    return { isValid: false }
  }
}

/**
 * Check if a custom integration is valid (meaning both the path and the
 * arguments are valid).
 *
 * @param customIntegration The custom integration to validate
 */
export async function isValidCustomIntegration(
  customIntegration: ICustomIntegration
): Promise<boolean> {
  try {
    const pathResult = await validateCustomIntegrationPath(
      customIntegration.path
    )
    const argv = await parseCustomIntegrationArgumentsForRuntime(
      customIntegration.arguments
    )
    const targetPathPresent = checkTargetPathArgument(argv)
    return pathResult.isValid && targetPathPresent
  } catch (e) {
    log.error('Failed to validate custom integration:', e)
    return false
  }
}

/**
 * Migrates custom integrations stored with the old format (with the arguments
 * stored as an array of strings) to the new format (with the arguments stored
 * as a single string).
 *
 * @param customIntegration The custom integration to migrate
 *
 * @returns The migrated custom integration, or `null` if the custom integration
 *         is already in the new format.
 */
export function migratedCustomIntegration(
  customIntegration: ICustomIntegration | null
): ICustomIntegration | null {
  if (customIntegration === null) {
    return null
  }

  // The first public release of the custom integrations feature stored the
  // arguments as an array of strings. This caused some issues because the
  // APIs used to parse them and split them into an array would remove any
  // quotes. Storing exactly the same string as the user entered and then parse
  // it right before invoking the custom integration is a better approach.
  if (!Array.isArray(customIntegration.arguments)) {
    return null
  }

  return {
    ...customIntegration,
    arguments: customIntegration.arguments.join(' '),
  }
}

/**
 * This helper function will use spawn to launch an integration (editor or shell).
 * Its main purpose is to do some platform-specific argument handling, for example
 * on Windows, where we need to wrap the command and arguments in quotes when
 * the shell option is enabled.
 *
 * @param cmd Command to spawn
 * @param args Arguments to pass to the command
 * @param options Options to pass to spawn (optional)
 * @returns The ChildProcess object returned by spawn
 */
export const spawnCustomIntegration = (
  cmd: string,
  args: readonly string[],
  opts?: Pick<ISpawnOptions, 'cwd' | 'env'>
): IChildProcess =>
  spawn(cmd, args, {
    stdio: 'ignore',
    detached: true,
    ...opts,
  }) as unknown as IChildProcess
