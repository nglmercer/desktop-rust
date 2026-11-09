// `@github/copilot-sdk` for the webview build. The SDK drives a Copilot CLI
// child process over stdio, which a webview cannot spawn. Until that runtime
// is hosted by the Rust shell, constructing a client fails with a clear error
// so Copilot features report themselves as unavailable instead of crashing
// the renderer at module load.
const unavailable = () =>
  new Error('GitHub Copilot is not available in the Rust shell yet')

export class RuntimeConnection {
  public static forStdio(_options?: unknown): RuntimeConnection {
    return new RuntimeConnection()
  }
}

export class CopilotClient {
  public constructor(_options?: unknown) {
    throw unavailable()
  }
}

export class CopilotSession {
  private constructor() {
    throw unavailable()
  }
}
