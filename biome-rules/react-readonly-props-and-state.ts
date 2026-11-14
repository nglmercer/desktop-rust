import { ast, defineRule, registerDiagnostic } from '@biomejs/runtime/plugin'

/** Preserve the original rule's filename, interface and type-text matching. */
export const reactReadonlyPropsAndState = defineRule({
  query: ast('TS_INTERFACE_DECLARATION'),
  run(node, context) {
    if (context.filePath.toLowerCase().endsWith('ts')) return
    const name =
      node.id?.kind === 'TS_IDENTIFIER_BINDING'
        ? node.id.nameToken?.replace(
            /\\u(?:\{([\da-fA-F]+)\}|([\da-fA-F]{4}))/g,
            (_match, codePoint: string | undefined, unit: string | undefined) =>
              String.fromCodePoint(Number.parseInt(codePoint ?? unit ?? '', 16))
          )
        : undefined
    if (!name?.endsWith('Props') && !name?.endsWith('State')) return

    for (const member of node.members) {
      if (member.kind !== 'TS_PROPERTY_SIGNATURE_TYPE_MEMBER') continue
      if (!member.readonlyToken) {
        registerDiagnostic(
          member,
          'error',
          'Property and state signatures should be read-only'
        )
      }
      const annotation = member.typeAnnotation?.text.trim()
      if (
        annotation &&
        (/^\: \s*Array<.*>$/.test(annotation) || annotation.endsWith('[]'))
      ) {
        registerDiagnostic(
          member,
          'error',
          'Prop and State arrays should be read only (ReadOnlyArray)'
        )
      }
    }
  },
})
