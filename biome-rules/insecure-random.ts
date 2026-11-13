import {
  ast,
  defineRule,
  registerDiagnostic,
  type AnyJsExpression,
} from '@biomejs/runtime/plugin'

/** ESLint's parser omits parentheses from its AST; Biome represents them. */
function unparenthesize(
  node: AnyJsExpression | undefined
): AnyJsExpression | undefined {
  while (node?.kind === 'JS_PARENTHESIZED_EXPRESSION') {
    node = node.expression
  }
  return node
}

/** Match ESLint's decoded identifier names, including Unicode escapes. */
function identifier(token: string | undefined): string | undefined {
  return token?.replace(
    /\\u(?:\{([\da-fA-F]+)\}|([\da-fA-F]{4}))/g,
    (_match, codePoint: string | undefined, unit: string | undefined) =>
      String.fromCodePoint(Number.parseInt(codePoint ?? unit ?? '', 16))
  )
}

export const insecureRandom = defineRule({
  query: ast('JS_CALL_EXPRESSION'),
  run(node) {
    const callee = unparenthesize(node.callee)
    if (!callee) return
    if (callee.kind === 'JS_IDENTIFIER_EXPRESSION') {
      if (identifier(callee.name?.valueToken) === 'pseudoRandomBytes') {
        registerDiagnostic(
          node,
          'error',
          'crypto.pseudoRandomBytes produces insecure random numbers. Use crypto.randomBytes() instead'
        )
      }
      return
    }
    if (
      callee.kind !== 'JS_STATIC_MEMBER_EXPRESSION' &&
      callee.kind !== 'JS_COMPUTED_MEMBER_EXPRESSION'
    )
      return
    const computed =
      callee.kind === 'JS_COMPUTED_MEMBER_EXPRESSION'
        ? unparenthesize(callee.member)
        : undefined
    const property =
      callee.kind === 'JS_STATIC_MEMBER_EXPRESSION'
        ? callee.member?.kind === 'JS_NAME'
          ? identifier(callee.member.valueToken)
          : undefined
        : computed?.kind === 'JS_IDENTIFIER_EXPRESSION'
        ? identifier(computed.name?.valueToken)
        : undefined
    const object = unparenthesize(callee.object)
    if (
      property === 'random' &&
      object?.kind === 'JS_IDENTIFIER_EXPRESSION' &&
      identifier(object.name?.valueToken) === 'Math'
    ) {
      registerDiagnostic(
        node,
        'error',
        'Math.random produces insecure random numbers. Use crypto.randomBytes() or window.crypto.getRandomValues() instead'
      )
    } else if (property === 'pseudoRandomBytes') {
      registerDiagnostic(
        node,
        'error',
        'crypto.pseudoRandomBytes produces insecure random numbers. Use crypto.randomBytes() instead'
      )
    }
  },
})
