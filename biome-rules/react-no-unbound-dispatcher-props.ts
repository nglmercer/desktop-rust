import { ast, defineRule, registerDiagnostic } from '@biomejs/runtime/plugin'

/** Match the legacy rule's exact JSX expression prefix, including spacing. */
export const reactNoUnboundDispatcherProps = defineRule({
  query: ast('JSX_EXPRESSION_ATTRIBUTE_VALUE', 'JSX_EXPRESSION_CHILD'),
  run(node) {
    const text = node.text.trimStart()
    if (/^\{this\.props\.dispatcher\./.test(text)) {
      const expression = text.slice(1, text.lastIndexOf('}'))
      registerDiagnostic(
        node,
        'error',
        `Use of unbound dispatcher method: ${expression}. Consider extracting the method call to a bound instance method.`
      )
    }
  },
})
