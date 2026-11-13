import {
  ast,
  defineRule,
  registerDiagnostic,
  type AnyJsExpression,
} from '@biomejs/runtime/plugin'

// ESTree drops parentheses and decodes identifier escapes. Preserve the
// behavior of the old rule, including computed identifier members.
function unparenthesize(
  node: AnyJsExpression | undefined
): AnyJsExpression | undefined {
  while (node?.kind === 'JS_PARENTHESIZED_EXPRESSION') node = node.expression
  return node
}

function identifier(token: string | undefined): string | undefined {
  return token?.replace(
    /\\u(?:\{([\da-fA-F]+)\}|([\da-fA-F]{4}))/g,
    (_match, codePoint: string | undefined, unit: string | undefined) =>
      String.fromCodePoint(Number.parseInt(codePoint ?? unit ?? '', 16))
  )
}

function identifierExpression(node: AnyJsExpression | undefined) {
  const expression = unparenthesize(node)
  return expression?.kind === 'JS_IDENTIFIER_EXPRESSION'
    ? identifier(expression.name?.valueToken)
    : undefined
}

function property(node: AnyJsExpression | undefined): string | undefined {
  const expression = unparenthesize(node)
  if (expression?.kind === 'JS_STATIC_MEMBER_EXPRESSION') {
    return expression.member?.kind === 'JS_NAME'
      ? identifier(expression.member.valueToken)
      : undefined
  }
  if (expression?.kind === 'JS_COMPUTED_MEMBER_EXPRESSION') {
    return identifierExpression(expression.member)
  }
  return undefined
}

export const noLooselyTypedWebContentsIpc = defineRule({
  query: ast('JS_CALL_EXPRESSION'),
  run(node) {
    const callee = unparenthesize(node.callee)
    if (
      (callee?.kind !== 'JS_STATIC_MEMBER_EXPRESSION' &&
        callee?.kind !== 'JS_COMPUTED_MEMBER_EXPRESSION') ||
      property(callee) !== 'send'
    ) {
      return
    }
    const object = unparenthesize(callee.object)
    const name = identifierExpression(object)
    if (
      name === 'wc' ||
      name === 'webContents' ||
      property(object) === 'webContents'
    ) {
      registerDiagnostic(
        node,
        'error',
        'Please use the strongly typed IPC helper methods from `ipc-webcontents` instead'
      )
    }
  },
})
