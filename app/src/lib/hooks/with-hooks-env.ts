import { cp, mkdtemp, rm } from '../../web/fs-promises'
import { hostTemporaryDirectory } from '../host-environment'
import { join } from 'path'
import { createProxyProcessServer, getProxyCommandPath } from 'process-proxy'
import type { IGitExecutionOptions } from '../git/core'
import { getRepoHooks, getRepoHooksPath } from './get-repo-hooks'
import { createHooksProxy } from './hooks-proxy'
import { getShellEnv } from './get-shell-env'
import { quoteGitConfigParameter } from './git-config-parameters'
import { memoizeOne } from '../memoize-one'
import {
  getCacheHooksEnv,
  getGitHookEnvShell,
  getHooksEnvEnabled,
  type SupportedHooksEnvShell,
} from './config'

const memoizedGetShellEnv = memoizeOne(
  async (shellKind: SupportedHooksEnvShell, cwd: string, cacheKey: string) => {
    const shellEnvStartTime = Date.now()
    const shellEnv = await getShellEnv(cwd, shellKind)
    log.debug(
      `hooks: loaded shell environment in ${Date.now() - shellEnvStartTime}ms`
    )
    return shellEnv
  }
)

type HookProxyAddress = {
  readonly port: number
}

export type RendererHookEnvironment = {
  readonly config_parameters: string
  readonly process_proxy_port: number
  readonly process_proxy_token: string
}

/** Convert the ephemeral process-proxy environment into the typed host contract. */
export function rendererHookEnvironment(
  environment: Record<string, string | undefined> | undefined
): RendererHookEnvironment | undefined {
  const token = environment?.PROCESS_PROXY_TOKEN
  const rawPort = environment?.PROCESS_PROXY_PORT
  if (token === undefined && rawPort === undefined) {
    return undefined
  }
  const port = Number(rawPort)
  if (
    token === undefined ||
    token.length === 0 ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > 65535
  ) {
    throw new TypeError('Invalid hook interception environment')
  }
  return {
    config_parameters: environment?.GIT_CONFIG_PARAMETERS ?? '',
    process_proxy_port: port,
    process_proxy_token: token,
  }
}

export async function withHooksEnv<T>(
  fn: (env: Record<string, string | undefined> | undefined) => Promise<T>,
  path: string,
  opts: IGitExecutionOptions | undefined
): Promise<T> {
  if (!opts?.interceptHooks || !getHooksEnvEnabled()) {
    return fn(opts?.env)
  }

  const hooks = await Array.fromAsync(
    getRepoHooks(path, opts.interceptHooks, opts.env)
  )

  if (hooks.length === 0) {
    return fn(opts?.env)
  }

  const originalHooksPath = await getRepoHooksPath(path, opts.env)

  const ext = __WIN32__ ? '.exe' : ''
  const processProxyPath = getProxyCommandPath()

  const token = crypto.randomUUID()
  const tmpHooksDir = await mkdtemp(
    join(hostTemporaryDirectory(), 'desktop-git-hooks-')
  )
  const hooksProxy = createHooksProxy(
    cwd =>
      memoizedGetShellEnv(
        getGitHookEnvShell(),
        cwd,
        // We always cache environment per token (i.e. per operation, e.g commit, apply, etc)
        // but we can optionally cache it over multiple operations in the same repository if the user
        // has enabled that setting.
        getCacheHooksEnv() ? 'global' : token
      ),
    tmpHooksDir,
    opts?.onHookProgress,
    opts?.onHookFailure,
    opts?.env?.GIT_CONFIG_PARAMETERS,
    originalHooksPath
  )

  const server = createProxyProcessServer(
    conn =>
      hooksProxy(conn).catch(err => {
        log.error(`hooks proxy failed:`, err)
        conn.exit(1).catch(() => {})
      }),
    { validateConnection: async receivedToken => receivedToken === token }
  )
  const port = await new Promise<number>(resolve => {
    server.listen(0, '127.0.0.1', () =>
      resolve((server.address() as HookProxyAddress).port)
    )
  })
  try {
    for (const hook of hooks) {
      await cp(processProxyPath, join(tmpHooksDir, `${hook}${ext}`))
    }

    const existingGitEnvConfig =
      opts?.env?.['GIT_CONFIG_PARAMETERS'] ??
      process.env['GIT_CONFIG_PARAMETERS'] ??
      ''

    const gitEnvConfigPrefix =
      existingGitEnvConfig.length > 0 ? `${existingGitEnvConfig} ` : ''

    return await fn({
      GIT_CONFIG_PARAMETERS: `${gitEnvConfigPrefix}${quoteGitConfigParameter(`core.hooksPath=${tmpHooksDir}`)}`,
      PROCESS_PROXY_PORT: `${port}`,
      PROCESS_PROXY_TOKEN: token,
    })
  } finally {
    server.close()
    // Clean up the temporary directory
    await rm(tmpHooksDir, { recursive: true, force: true }).catch(() => {
      // Ignore errors
    })
  }
}
