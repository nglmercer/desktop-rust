// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

interface Fixture {
  filename: string
  code: string
  errors?: ReadonlyArray<{ messageId: string; data: Record<string, string> }>
}

function lint(fixture: Fixture) {
  const root = resolve('.')
  const dir = mkdtempSync(join(root, 'app/test/biome-lifecycle-'))
  try {
    const file = join(
      dir,
      fixture.filename.endsWith('.ts') ? 'input.ts' : 'input.tsx'
    )
    writeFileSync(file, fixture.code)
    const result = spawnSync(
      process.execPath,
      [
        resolve('node_modules/@biomejs/biome/bin/biome'),
        'lint',
        '--config-path=biome.security.json',
        file,
        '--reporter=json',
      ],
      { cwd: root, encoding: 'utf8' }
    )
    if (result.error) throw result.error
    const report = JSON.parse(result.stdout) as {
      diagnostics: Array<{ severity: string; message: string }>
    }
    return report.diagnostics
      .filter(item => item.severity === 'error')
      .map(item => item.message)
      .sort()
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function expected(error: NonNullable<Fixture['errors']>[number]): string {
  const { methodName, parameterName, expectedName, expectedType } = error.data
  switch (error.messageId) {
    case 'emptyParametersExpected':
      return `${methodName} should not accept any parameters`
    case 'unknownParameter':
      return `${methodName} has unknown parameter ${parameterName}`
    case 'nameMismatch':
      return `${methodName} has parameter ${parameterName} which does not match expected name ${expectedName}`
    case 'typeMismatch':
      return `${methodName} has parameter ${parameterName} which does not match expected type ${expectedType}`
    case 'reservedMethodName':
      return `Method name ${methodName} is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods`
    default:
      throw new Error(`Unknown legacy diagnostic: ${error.messageId}`)
  }
}

// Retain the fixtures and diagnostics from the former ESLint rule.
const ruleTester = {
  run(
    _name: string,
    cases: { valid: ReadonlyArray<Fixture>; invalid: ReadonlyArray<Fixture> }
  ) {
    it.each(cases.valid.map((fixture, index) => [index, fixture] as const))(
      'accepts valid fixture %s',
      (_index, fixture) => {
        expect(lint(fixture)).toEqual([])
      }
    )
    it.each(cases.invalid.map((fixture, index) => [index, fixture] as const))(
      'reports invalid fixture %s',
      (_index, fixture) => {
        expect(lint(fixture)).toEqual(
          (fixture.errors ?? []).map(expected).sort()
        )
      }
    )
  },
}

describe('react-proper-lifecycle-methods', () => {
  ruleTester.run('react-proper-lifecycle-methods', {
    valid: [
      // component without lifecycle methods passes without errors
      {
        filename: 'app/src/component-no-state.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps> {
    public render() {
      return null
    }
}
`,
      },
      {
        filename: 'app/src/component-lifecycle-events-no-state.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps> {
    public componentWillMount() {}

    public componentDidMount() {}

    public componentWillUnmount() {}

    public componentWillUpdate(nextProps: ICloningRepositoryProps) {}

    public componentDidUpdate(prevProps: ICloningRepositoryProps) {}

    public render() {
        return null
    }
}
`,
      },
      {
        filename: 'app/src/component-lifecycle-events-with-state.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public componentWillMount() {}

    public componentDidMount() {}

    public componentWillUnmount() {}

    public componentWillUpdate(nextProps: ICloningRepositoryProps, nextState: ICloningRepositoryState) {}

    public componentDidUpdate(prevProps: ICloningRepositoryProps, prevState: ICloningRepositoryState) {}

    public render() {
      return null
    }
}
`,
      },
      {
        filename: 'app/src/pure-component-lifecycle-events-with-state.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.PureComponent<ICloningRepositoryProps,ICloningRepositoryState> {
    public componentWillMount() {}

    public componentDidMount() {}

    public componentWillUnmount() {}

    public componentWillUpdate(nextProps: ICloningRepositoryProps, nextState: ICloningRepositoryState) {}

    public componentDidUpdate(prevProps: ICloningRepositoryProps, prevState: ICloningRepositoryState) {}

    public render() {
      return null
    }
}
`,
      },
      {
        filename: 'app/src/component-lifecycle-events-with-type-literal.tsx',
        code: `
import * as React from 'react'

interface IWindowControlState {}

export class WindowControls extends React.Component<{}, IWindowControlState> {

  public shouldComponentUpdate(nextProps: {}, nextState: IWindowControlState) {
    return nextState.windowState !== this.state.windowState
  }

  public componentWillUpdate(nextProps: {}, nextState: IWindowControlState) { }

  public render() {
    return null
  }
}
`,
      },
      {
        filename: 'app/src/component-lifecycle-events-with-type-literal.tsx',
        code: `
import * as React from 'react'

interface IWindowControlState {}

export class WindowControls extends React.Component {

    public shouldComponentUpdate(nextProps: {}) {
        return nextState.windowState !== this.state.windowState
    }

    public componentWillUpdate(nextProps: {}) { }

    public render() {
        return null
    }
}
`,
      },
      // a regular class with the same method name is ignored
      {
        filename: 'app/src/ui/other.tsx',
        code: `
  class Something {
    public componentWillUpdate(foo: string) {

    }
  }
  `,
      },
    ],
    invalid: [
      //
      // shouldComponentUpdate expects the first parameter to be nextProps and match the component's prop type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps> {
    public shouldComponentUpdate(foo: string) { return true }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'shouldComponentUpdate',
              parameterName: 'foo',
              expectedName: 'nextProps',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'shouldComponentUpdate',
              parameterName: 'foo',
              expectedType: 'ICloningRepositoryProps',
            },
          },
        ],
      },
      //
      // shouldComponentUpdate expects the second parameter to be nextState and match the component's state type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public shouldComponentUpdate(nextProps: ICloningRepositoryProps, foo: string) { return true }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'shouldComponentUpdate',
              parameterName: 'foo',
              expectedName: 'nextState',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'shouldComponentUpdate',
              parameterName: 'foo',
              expectedType: 'ICloningRepositoryState',
            },
          },
        ],
      },
      //
      // shouldComponentUpdate is not permitted to have any additional parameters
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public shouldComponentUpdate(nextProps: ICloningRepositoryProps, nextState: ICloningRepositoryState, additionalParam: void) { return true }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'unknownParameter',
            data: {
              methodName: 'shouldComponentUpdate',
              parameterName: 'additionalParam',
            },
          },
        ],
      },

      //
      // componentWillUpdate expects the first parameter to be nextProps and match the component's prop type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps> {
    public componentWillUpdate(bar: string) { }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'bar',
              expectedName: 'nextProps',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'bar',
              expectedType: 'ICloningRepositoryProps',
            },
          },
        ],
      },

      //
      // componentWillUpdate expects the first parameter to be nextProps and match the component's type literal
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface IWindowControlState {}

export class CloningRepositoryView extends React.Component<{}, IWindowControlState> {
    public componentWillUpdate(bar: string) { }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'bar',
              expectedName: 'nextProps',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'bar',
              expectedType: '{}',
            },
          },
        ],
      },

      //
      // componentWillUpdate expects the second parameter to be nextState and match the component's state type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public componentWillUpdate(nextProps: ICloningRepositoryProps, foo: string) {  }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'foo',
              expectedName: 'nextState',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'componentWillUpdate',
              parameterName: 'foo',
              expectedType: 'ICloningRepositoryState',
            },
          },
        ],
      },

      //
      // componentWillUpdate expects the first parameter to be nextProps and match the component's type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface IWindowControlProps {}

interface IWindowControlState {}

export class CloningRepositoryView extends React.Component<IWindowControlProps, IWindowControlState> {
    public componentDidUpdate(bar: string) { }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'componentDidUpdate',
              parameterName: 'bar',
              expectedName: 'prevProps',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'componentDidUpdate',
              parameterName: 'bar',
              expectedType: 'IWindowControlProps',
            },
          },
        ],
      },

      //
      // componentDidUpdate expects the second parameter to be nextState and match the component's state type
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public componentDidUpdate(prevProps: ICloningRepositoryProps, foo: string) {  }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'nameMismatch',
            data: {
              methodName: 'componentDidUpdate',
              parameterName: 'foo',
              expectedName: 'prevState',
            },
          },
          {
            messageId: 'typeMismatch',
            data: {
              methodName: 'componentDidUpdate',
              parameterName: 'foo',
              expectedType: 'ICloningRepositoryState',
            },
          },
        ],
      },

      //
      // Methods inside a component following the `component*` or `shouldComponent*` pattern should be rejected
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

interface ICloningRepositoryProps {}

interface ICloningRepositoryState {}

export class CloningRepositoryView extends React.Component<ICloningRepositoryProps,ICloningRepositoryState> {
    public componentWillDoSomething() { }

    public shouldComponentFoo() { return false }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'reservedMethodName',
            data: {
              methodName: 'componentWillDoSomething',
            },
          },
          {
            messageId: 'reservedMethodName',
            data: {
              methodName: 'shouldComponentFoo',
            },
          },
        ],
      },

      //
      // Methods inside a non-generic component following the `component*` or `shouldComponent*` pattern should be rejected
      //
      {
        filename: 'app/src/component.tsx',
        code: `
import * as React from 'react'

export class CloningRepositoryView extends React.Component {
    public componentWillDoSomething() { }

    public shouldComponentFoo() { return false }

    public render() {
        return null
    }
}
`,
        errors: [
          {
            messageId: 'reservedMethodName',
            data: {
              methodName: 'componentWillDoSomething',
            },
          },
          {
            messageId: 'reservedMethodName',
            data: {
              methodName: 'shouldComponentFoo',
            },
          },
        ],
      },
    ],
  })
})

describe('Biome lifecycle validation', () => {
  it('checks parenthesized React bases and computed method names', () => {
    for (const base of [
      '(React.Component)',
      '(React).Component',
      'React[(Component)]',
    ]) {
      expect(
        lint({
          filename: 'component.tsx',
          code: `class C extends ${base} { [(componentBogus)]() {} }`,
        })
      ).toEqual([
        "Method name componentBogus is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
      ])
    }
  })

  it('checks getters, setters, overloads, computed names and Unicode', () => {
    const diagnostics = lint({
      filename: 'component.tsx',
      code: `class Component extends React[Component] {
        get componentBogus() { return 1 }
        set componentDidMount(value: string) {}
        [componentComputed]() {}
        componentÉvil() {}
        componentOverload(): void;
        componentOverload() {}
      }`,
    })
    expect(diagnostics).toEqual(
      [
        "Method name componentBogus is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
        "Method name componentComputed is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
        "Method name componentOverload is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
        "Method name componentOverload is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
        "Method name componentÉvil is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
        'componentDidMount should not accept any parameters',
      ].sort()
    )
  })

  it('normalizes escaped component, method, parameter and type names', () => {
    expect(
      lint({
        filename: 'component.tsx',
        code: `class Component extends React.\\u0043omponent<P\\u0072ops> {
        shouldComponentUpdate(next\\u0050rops: Props) {}
        component\\u0042ogus() {}
      }`,
      })
    ).toEqual([
      "Method name componentBogus is prohibited as names starting with 'component' or 'shouldComponent' can be confused with React lifecycle methods",
    ])
  })

  it('checks default-exported classes and reports unsupported parameter types', () => {
    expect(
      lint({
        filename: 'component.tsx',
        code: `export default class Component extends React.Component<Props> {
        componentDidUpdate(prevProps) {}
      }`,
      })
    ).toEqual([
      'componentDidUpdate has a parameter with an unsupported or missing type annotation',
    ])
  })

  it('does not apply component state to a subsequent unrelated class', () => {
    expect(
      lint({
        filename: 'component.tsx',
        code: `class Component extends React.Component<Props> {}
        class Other { componentWillUpdate(value: string) {} }`,
      })
    ).toEqual([])
  })

  it('retains the original exclusion for .ts files', () => {
    expect(
      lint({
        filename: 'component.ts',
        code: 'class Component extends React.Component { componentInvalid() {} }',
      })
    ).toEqual([])
  })
})
