import {
  ast,
  defineRule,
  registerDiagnostic,
  type AnyJsExpression,
} from '@biomejs/runtime/plugin'

function identifier(value: string | undefined): string {
  return (value ?? '').replace(
    /\\u(?:\{([\da-fA-F]+)\}|([\da-fA-F]{4}))/g,
    (_match, codePoint: string | undefined, unit: string | undefined) =>
      String.fromCodePoint(Number.parseInt(codePoint ?? unit ?? '', 16))
  )
}

function unparenthesize(
  expression: AnyJsExpression | undefined
): AnyJsExpression | undefined {
  while (expression?.kind === 'JS_PARENTHESIZED_EXPRESSION')
    expression = expression.expression
  return expression
}

/** Check the lifecycle signatures of class components. */
export const reactProperLifecycleMethods = defineRule({
  query: ast('JS_CLASS_DECLARATION', 'JS_CLASS_EXPORT_DEFAULT_DECLARATION'),
  run(node, context) {
    if (context.filePath.toLowerCase().endsWith('ts')) return
    const base = unparenthesize(node.extendsClause?.superClass)
    if (
      base?.kind !== 'JS_STATIC_MEMBER_EXPRESSION' &&
      base?.kind !== 'JS_COMPUTED_MEMBER_EXPRESSION'
    )
      return
    const computedBase =
      base.kind === 'JS_COMPUTED_MEMBER_EXPRESSION'
        ? unparenthesize(base.member)
        : undefined
    const baseObject = unparenthesize(base.object)
    const baseName =
      base.kind === 'JS_STATIC_MEMBER_EXPRESSION'
        ? base.member?.text.trim()
        : computedBase?.kind === 'JS_IDENTIFIER_EXPRESSION'
        ? computedBase.name?.valueToken
        : undefined
    if (
      baseObject?.kind !== 'JS_IDENTIFIER_EXPRESSION' ||
      identifier(baseObject.name?.valueToken) !== 'React' ||
      !['Component', 'PureComponent'].includes(identifier(baseName))
    )
      return

    const types = node.extendsClause?.typeArguments?.tsTypeArgumentList
    const typeName = (index: number): string | null => {
      if (!types) return '{}'
      const type = types[index]
      if (
        type?.kind === 'TS_REFERENCE_TYPE' &&
        type.name?.kind === 'JS_REFERENCE_IDENTIFIER'
      )
        return identifier(type.name.valueToken)
      if (type?.kind === 'TS_OBJECT_TYPE') {
        if (index === 1) return type.text.trim()
        if (type.members.length === 0) return '{}'
      }
      return null
    }
    for (const member of node.members) {
      if (
        member.kind !== 'JS_METHOD_CLASS_MEMBER' &&
        member.kind !== 'TS_METHOD_SIGNATURE_CLASS_MEMBER' &&
        member.kind !== 'JS_GETTER_CLASS_MEMBER' &&
        member.kind !== 'TS_GETTER_SIGNATURE_CLASS_MEMBER' &&
        member.kind !== 'JS_SETTER_CLASS_MEMBER' &&
        member.kind !== 'TS_SETTER_SIGNATURE_CLASS_MEMBER'
      )
        continue
      const computedName =
        member.name?.kind === 'JS_COMPUTED_MEMBER_NAME'
          ? unparenthesize(member.name.expression)
          : undefined
      const rawName =
        member.name?.kind === 'JS_LITERAL_MEMBER_NAME'
          ? member.name.value
          : computedName?.kind === 'JS_IDENTIFIER_EXPRESSION'
          ? computedName.name?.valueToken
          : undefined
      const name = identifier(rawName)
      if (
        !name ||
        (!name.startsWith('component') && !name.startsWith('shouldComponent'))
      )
        continue
      const params =
        member.kind === 'JS_METHOD_CLASS_MEMBER' ||
        member.kind === 'TS_METHOD_SIGNATURE_CLASS_MEMBER'
          ? member.parameters?.items ?? []
          : member.kind === 'JS_SETTER_CLASS_MEMBER' ||
            member.kind === 'TS_SETTER_SIGNATURE_CLASS_MEMBER'
          ? member.parameter
            ? [member.parameter]
            : []
          : []
      if (
        [
          'componentWillMount',
          'componentDidMount',
          'componentWillUnmount',
        ].includes(name)
      ) {
        if (params.length)
          registerDiagnostic(
            member,
            'error',
            `${name} should not accept any parameters`
          )
        continue
      }
      const names =
        name === 'componentDidUpdate'
          ? ['prevProps', 'prevState']
          : ['nextProps', 'nextState']
      if (
        ![
          'componentWillReceiveProps',
          'componentWillUpdate',
          'componentDidUpdate',
          'shouldComponentUpdate',
        ].includes(name)
      ) {
        registerDiagnostic(
          member,
          'error',
          `Method name ${name} is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods`
        )
        continue
      }
      const max = name === 'componentWillReceiveProps' ? 1 : 2
      for (let i = 0; i < params.length; i++) {
        const parameter = params[i]
        if (
          parameter.kind !== 'JS_FORMAL_PARAMETER' ||
          parameter.binding?.kind !== 'JS_IDENTIFIER_BINDING'
        ) {
          registerDiagnostic(
            parameter,
            'error',
            `${name} requires a named, typed parameter`
          )
          break
        }
        const actual = identifier(parameter.binding.nameToken)
        if (i >= max) {
          registerDiagnostic(
            member,
            'error',
            `${name} has unknown parameter ${actual}`
          )
          break
        }
        let valid = true
        if (actual !== names[i]) {
          registerDiagnostic(
            parameter,
            'error',
            `${name} has parameter ${actual} which does not match expected name ${names[i]}`
          )
          valid = false
        }
        const annotation = parameter.typeAnnotation?.ty
        const actualType =
          annotation?.kind === 'TS_REFERENCE_TYPE' &&
          annotation.name?.kind === 'JS_REFERENCE_IDENTIFIER'
            ? identifier(annotation.name.valueToken)
            : annotation?.kind === 'TS_STRING_TYPE'
            ? 'string'
            : annotation?.kind === 'TS_OBJECT_TYPE' &&
              annotation.members.length === 0
            ? '{}'
            : undefined
        if (actualType === undefined) {
          registerDiagnostic(
            parameter,
            'error',
            `${name} has a parameter with an unsupported or missing type annotation`
          )
          valid = false
        } else if (actualType !== typeName(i)) {
          registerDiagnostic(
            parameter,
            'error',
            `${name} has parameter ${actual} which does not match expected type ${typeName(
              i
            )}`
          )
          valid = false
        }
        if (!valid) break
      }
    }
  },
})
