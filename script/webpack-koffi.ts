import webpack from 'webpack'

/** Bundle only the target's native Koffi package, even during cross-compilation. */
export function getKoffiWebpackConfig(
  nodeModulesRoot: string,
  platform: NodeJS.Platform,
  arch: string
): webpack.Configuration {
  const targetPackage = `@koromix/koffi-${platform}-${arch}`
  // Koffi catches missing optional packages. Resolve eagerly so a missing target
  // fails the build instead of producing an app that cannot load the SDK.
  let resolutionRoot = nodeModulesRoot
  try {
    const sdk = require.resolve('@github/copilot-sdk', {
      paths: [nodeModulesRoot],
    })
    resolutionRoot = require.resolve('koffi', { paths: [sdk] })
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'MODULE_NOT_FOUND')
      throw error
  }
  // Resolve from the Koffi instance loaded by the SDK. A root-level native
  // package can belong to another Koffi version when pnpm keeps both trees.
  // When the SDK is absent (e.g. an isolated bundler fixture), use the root.
  const entry = require.resolve(targetPackage, { paths: [resolutionRoot] })

  return {
    resolve: { alias: { [`${targetPackage}$`]: entry } },
    plugins: [
      new webpack.IgnorePlugin({
        checkResource: request =>
          request.startsWith('@koromix/koffi-') && request !== targetPackage,
      }),
    ],
  }
}
