/**
 * Project-specific Oxlint rules.
 *
 * These rules intentionally preserve the behavior of the historical Desktop
 * checks, including a few odd edges encoded by their regression tests. They
 * are syntax-only rules: TypeScript 7 performs type checking separately.
 */

function sourceCodeOf(context) {
  return context.sourceCode ?? context.getSourceCode()
}

function filenameOf(context) {
  return context.filename ?? context.getFilename()
}

function identifierName(node) {
  return node?.type === 'Identifier' ? node.name : undefined
}

/**
 * The legacy rules treated computed identifiers as member names but not string
 * literals: `obj[member]()` matched while `obj['member']()` did not.
 */
function memberPropertyName(node) {
  if (node?.type !== 'MemberExpression') return undefined
  if (!node.computed && node.property.type === 'Identifier') {
    return node.property.name
  }
  if (node.computed && node.property.type === 'Identifier') {
    return node.property.name
  }
  return undefined
}

function report(context, node, message) {
  context.report({ node, message })
}

const insecureRandom = {
  create(context) {
    return {
      CallExpression(node) {
        const callee = node.callee
        if (callee.type === 'Identifier') {
          if (callee.name === 'pseudoRandomBytes') {
            report(
              context,
              node,
              'crypto.pseudoRandomBytes produces insecure random numbers. Use crypto.randomBytes() instead'
            )
          }
          return
        }

        if (callee.type !== 'MemberExpression') return
        const property = memberPropertyName(callee)
        if (
          property === 'random' &&
          callee.object.type === 'Identifier' &&
          callee.object.name === 'Math'
        ) {
          report(
            context,
            node,
            'Math.random produces insecure random numbers. Use crypto.randomBytes() or window.crypto.getRandomValues() instead'
          )
        } else if (property === 'pseudoRandomBytes') {
          report(
            context,
            node,
            'crypto.pseudoRandomBytes produces insecure random numbers. Use crypto.randomBytes() instead'
          )
        }
      },
    }
  },
}

const noLooselyTypedWebcontentsIpc = {
  create(context) {
    return {
      CallExpression(node) {
        const callee = node.callee
        if (
          callee.type !== 'MemberExpression' ||
          memberPropertyName(callee) !== 'send'
        ) {
          return
        }

        const object = callee.object
        const objectName = identifierName(object)
        if (
          objectName === 'wc' ||
          objectName === 'webContents' ||
          memberPropertyName(object) === 'webContents'
        ) {
          report(
            context,
            node,
            'Please use the strongly typed IPC helper methods from `ipc-webcontents` instead'
          )
        }
      },
    }
  },
}

const reactNoUnboundDispatcherProps = {
  create(context) {
    const sourceCode = sourceCodeOf(context)
    return {
      JSXExpressionContainer(node) {
        const text = sourceCode.text.slice(node.start, node.end).trimStart()
        if (!/^\{this\.props\.dispatcher\./.test(text)) return
        const expression = text.slice(1, text.lastIndexOf('}'))
        report(
          context,
          node,
          `Use of unbound dispatcher method: ${expression}. Consider extracting the method call to a bound instance method.`
        )
      },
    }
  },
}

const reactReadonlyPropsAndState = {
  create(context) {
    if (filenameOf(context).toLowerCase().endsWith('ts')) return {}
    const sourceCode = sourceCodeOf(context)
    return {
      TSInterfaceDeclaration(node) {
        const name = identifierName(node.id)
        if (!name?.endsWith('Props') && !name?.endsWith('State')) return

        for (const member of node.body.body) {
          if (member.type !== 'TSPropertySignature') continue
          const annotation = member.typeAnnotation
            ? sourceCode.text
                .slice(member.typeAnnotation.start, member.typeAnnotation.end)
                .trim()
            : undefined
          if (
            annotation &&
            (/^\: \s*Array<.*>$/.test(annotation) || annotation.endsWith('[]'))
          ) {
            report(
              context,
              member,
              'Prop and State arrays should be read only (ReadOnlyArray)'
            )
          }
          if (!member.readonly) {
            report(
              context,
              member,
              'Property and state signatures should be read-only'
            )
          }
        }
      },
    }
  },
}

function typeReferenceName(type) {
  if (
    type?.type === 'TSTypeReference' &&
    type.typeName?.type === 'Identifier'
  ) {
    return type.typeName.name
  }
  return undefined
}

function classTypeArgument(node, index, sourceCode) {
  const type = node.superTypeArguments?.params[index]
  if (!type) return '{}'
  const reference = typeReferenceName(type)
  if (reference !== undefined) return reference
  if (type.type === 'TSTypeLiteral') {
    if (index === 1) {
      return sourceCode.text.slice(type.start, type.end).trim()
    }
    if (type.members.length === 0) return '{}'
  }
  return null
}

function parameterTypeName(parameter) {
  const type = parameter.typeAnnotation?.typeAnnotation
  const reference = typeReferenceName(type)
  if (reference !== undefined) return reference
  if (type?.type === 'TSStringKeyword') return 'string'
  if (type?.type === 'TSTypeLiteral' && type.members.length === 0) return '{}'
  return undefined
}

function methodName(member) {
  if (member.key?.type === 'Identifier') return member.key.name
  if (member.key?.type === 'Literal') return String(member.key.value)
  return undefined
}

function methodParameters(member) {
  return member.value?.params ?? []
}

const reactProperLifecycleMethods = {
  create(context) {
    if (filenameOf(context).toLowerCase().endsWith('ts')) return {}
    const sourceCode = sourceCodeOf(context)

    return {
      ClassDeclaration(node) {
        const base = node.superClass
        if (
          base?.type !== 'MemberExpression' ||
          identifierName(base.object) !== 'React' ||
          !['Component', 'PureComponent'].includes(memberPropertyName(base))
        ) {
          return
        }

        const expectedType = index => classTypeArgument(node, index, sourceCode)

        for (const member of node.body.body) {
          if (
            member.type !== 'MethodDefinition' &&
            member.type !== 'TSAbstractMethodDefinition'
          ) {
            continue
          }
          const name = methodName(member)
          if (
            !name ||
            (!name.startsWith('component') &&
              !name.startsWith('shouldComponent'))
          ) {
            continue
          }

          const params = methodParameters(member)
          if (
            [
              'componentWillMount',
              'componentDidMount',
              'componentWillUnmount',
            ].includes(name)
          ) {
            if (params.length) {
              report(
                context,
                member,
                `${name} should not accept any parameters`
              )
            }
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
            report(
              context,
              member,
              `Method name ${name} is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods`
            )
            continue
          }

          const max = name === 'componentWillReceiveProps' ? 1 : 2
          for (let i = 0; i < params.length; i++) {
            const parameter = params[i]
            if (parameter.type !== 'Identifier') {
              report(
                context,
                parameter,
                `${name} requires a named, typed parameter`
              )
              break
            }

            const actual = parameter.name
            if (i >= max) {
              report(context, member, `${name} has unknown parameter ${actual}`)
              break
            }

            let valid = true
            if (actual !== names[i]) {
              report(
                context,
                parameter,
                `${name} has parameter ${actual} which does not match expected name ${names[i]}`
              )
              valid = false
            }

            const actualType = parameterTypeName(parameter)
            if (actualType === undefined) {
              report(
                context,
                parameter,
                `${name} has a parameter with an unsupported or missing type annotation`
              )
              valid = false
            } else if (actualType !== expectedType(i)) {
              report(
                context,
                parameter,
                `${name} has parameter ${actual} which does not match expected type ${expectedType(i)}`
              )
              valid = false
            }
            if (!valid) break
          }
        }
      },
    }
  },
}

export default {
  meta: { name: 'desktop' },
  rules: {
    'insecure-random': insecureRandom,
    'no-loosely-typed-webcontents-ipc': noLooselyTypedWebcontentsIpc,
    'react-no-unbound-dispatcher-props': reactNoUnboundDispatcherProps,
    'react-readonly-props-and-state': reactReadonlyPropsAndState,
    'react-proper-lifecycle-methods': reactProperLifecycleMethods,
  },
}
