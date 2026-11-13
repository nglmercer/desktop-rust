// @vitest-environment node
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { rmSync, writeFileSync } from 'node:fs'
import { mkdtemp, mkdir, readFile, rm, writeFile, chmod } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { describe, expect, it } from 'vitest'

// This test temporarily replaces the ignored debug renderer index. Run it only
// with a built debug desktop binary and no concurrent Vite build.
describe.skipIf(process.env.DESKTOP_TEST_WRY_PROGRESS !== '1' || process.platform !== 'linux')(
  'native Wry commit progress isolation',
  () => {
    it('delivers two real commits only to their own RPC callbacks', async () => {
      const index = path.resolve('out/vite-dist/index.html')
      const original = await readFile(index)
      const root = await mkdtemp(path.join(tmpdir(), 'desktop-wry-progress-'))
      const marker = path.join(root, 'result.json')
      const repos = ['first', 'second'].map(name => path.join(root, name))
      let child: ChildProcess | undefined
      let spawnError: Error | undefined
      let replaced = false
      let logs = ''
      const restoreOnSignal = (code: number) => {
        if (child?.pid !== undefined) {
          try { process.kill(-child.pid, 'SIGKILL') } catch { /* Already exited. */ }
        }
        try { if (replaced) writeFileSync(index, original) } finally {
          rmSync(root, { recursive: true, force: true })
        }
        process.exit(code)
      }
      const onInterrupt = () => restoreOnSignal(130)
      const onTerminate = () => restoreOnSignal(143)
      process.once('SIGINT', onInterrupt)
      process.once('SIGTERM', onTerminate)
      const env = { ...process.env, HOME: root, XDG_DATA_HOME: path.join(root, 'data'), XDG_CONFIG_HOME: path.join(root, 'config'), XDG_CACHE_HOME: path.join(root, 'cache'), GIT_CONFIG_NOSYSTEM: '1', NO_COLOR: '1', WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS: '1' }
      for (const key of ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_CONFIG_COUNT', 'GIT_CONFIG_PARAMETERS']) delete env[key as keyof typeof env]
      const git = (repo: string, args: string[]) => {
        const result = spawnSync('git', ['-C', repo, ...args], { env, encoding: 'utf8', timeout: 10_000 })
        expect(result.error).toBeUndefined()
        expect(result.status, result.stderr).toBe(0)
      }
      try {
        for (const [i, repo] of repos.entries()) {
          await mkdir(repo)
          git(repo, ['init', '-q'])
          git(repo, ['config', 'user.name', 'Test'])
          git(repo, ['config', 'user.email', 'test@example.invalid'])
          git(repo, ['config', 'commit.gpgsign', 'false'])
          await writeFile(path.join(repo, 'file'), 'before\n')
          git(repo, ['add', 'file'])
          git(repo, ['commit', '-qm', 'initial'])
          await writeFile(path.join(repo, 'file'), 'after\n')
          const hook = path.join(repo, '.git/hooks/pre-commit')
          await writeFile(hook, `#!/bin/sh\necho ${i === 0 ? 'first' : 'second'}-hook-output >&2\nsleep 1\n`)
          await chmod(hook, 0o700)
        }
        const html = `<!doctype html><html><body><script>
        (async()=>{const repos=${JSON.stringify(repos)},marker=${JSON.stringify(marker)};let n=0;const global=[];
          window.__desktop.on('commit_output',(_,value)=>global.push(value));
          window.__desktop.on('commit_hook_progress',(_,value)=>global.push(value));
          const call=(repo,type,operation,callback)=>{const request={request_id:'00000000-0000-4000-8000-'+String(++n).padStart(12,'0'),deadline_ms:60000,operation:{type,repository_path:repo,operation}};return callback?window.__desktop.invokeWithProgress('desktop.backend.git_info',callback,request):window.__desktop.invoke('desktop.backend.git_info',request)};
          const prepare=async repo=>{const selection={paths:['file'],remove_paths:[],partial_selections:[],allow_empty:false,amend_head:null,expected_head:null};const review=await call(repo,'selected_commit_preview',{method:'selected_commit_preview',...selection});return (await call(repo,'commit_checkpoint',{method:'prepare_selected_commit',...selection,basis:review.basis})).id};
          let result;try{const ids=await Promise.all(repos.map(prepare)),events=[[],[]];const operation=i=>({method:'publish_selected_commit',checkpoint_id:ids[i],message:'native test\\n',no_verify:false,sign_off:false,allow_large_files:false});let oldRendererRejected=false;try{await call(repos[0],'commit_checkpoint',operation(0))}catch(error){oldRendererRejected=error&&error.code==='unsupported'}await Promise.all(repos.map((repo,i)=>call(repo,'commit_checkpoint',operation(i),event=>events[i].push(event))));const output=events.map(items=>items.filter(event=>event.event==='commit_output').map(event=>atob(event.data)).join(''));result={ids,events,output,global,oldRendererRejected};}catch(error){result={error:String(error),stack:error&&error.stack}}
          const encoded=btoa(Array.from(new TextEncoder().encode(JSON.stringify(result)),byte=>String.fromCharCode(byte)).join(''));await window.__desktop.invoke('desktop.io.fs-write-file',{path:marker,data:encoded});
        })();</script></body></html>`
        replaced = true
        await writeFile(index, html)
        child = spawn('xvfb-run', ['-a', path.resolve('rust/target/debug/github-desktop-rs')], { env, detached: true, stdio: ['ignore', 'pipe', 'pipe'] })
        child.once('error', error => { spawnError = error })
        child.stdout?.on('data', bytes => { logs += String(bytes) })
        child.stderr?.on('data', bytes => { logs += String(bytes) })
        const deadline = Date.now() + 60_000
        let result: any
        while (Date.now() < deadline) {
          if (spawnError) throw spawnError
          if (child.exitCode !== null) throw new Error(`Wry exited ${child.exitCode}: ${logs.slice(-4000)}`)
          try { result = JSON.parse(await readFile(marker, 'utf8')); break } catch (error) {
            if (!(error instanceof SyntaxError) && (error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
          }
          await delay(100)
        }
        expect(result, `Wry did not return a result: ${logs.slice(-4000)}`).toBeDefined()
        expect(result.error).toBeUndefined()
        expect(result.oldRendererRejected).toBe(true)
        expect(result.global).toEqual([])
        for (let i = 0; i < 2; i++) {
          expect(result.events[i].every((event: any) => event.checkpoint_id === result.ids[i])).toBe(true)
          expect(result.events[i]).toContainEqual(expect.objectContaining({ event: 'commit_hook_progress', hook_name: 'pre-commit', status: 'started' }))
          expect(result.events[i]).toContainEqual(expect.objectContaining({ event: 'commit_hook_progress', hook_name: 'pre-commit', status: 'finished' }))
          expect(result.output[i]).toContain(i === 0 ? 'first-hook-output' : 'second-hook-output')
          expect(result.output[i]).not.toContain(i === 0 ? 'second-hook-output' : 'first-hook-output')
        }
      } finally {
        process.off('SIGINT', onInterrupt)
        process.off('SIGTERM', onTerminate)
        if (child?.pid !== undefined && child.exitCode === null && child.signalCode === null) {
          const exited = new Promise<void>(resolve => child!.once('close', () => resolve()))
          try { process.kill(-child.pid, 'SIGKILL') } catch { /* Already exited. */ }
          await Promise.race([exited, delay(5_000)])
        }
        if (replaced) await writeFile(index, original)
        await rm(root, { recursive: true, force: true })
      }
    }, 90_000)
  }
)
