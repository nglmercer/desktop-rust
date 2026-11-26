import assert from 'node:assert'
import { resolve } from 'node:path'
import {
  API,
  TypeFlags,
  type Checker,
  type Project,
  type Type,
} from 'typescript/unstable/sync'
import { isTypeAliasDeclaration, SyntaxKind } from 'typescript/unstable/ast'

import { buildStatsPayload } from '../../src/lib/stats/stats-payload'
import type { StatsPayload } from '../../src/lib/stats/stats-store'

type ITelemetryPayload = ReturnType<typeof buildStatsPayload>

interface IStatsTypeContext {
  readonly api: API
  readonly project: Project
  readonly checker: Checker
  readonly type: Type
}

function getDailyStatsType(): IStatsTypeContext {
  const api = new API({ cwd: process.cwd() })
  try {
    const snapshot = api.updateSnapshot({
      openProjects: [resolve('tsconfig.json')],
    })
    const project = snapshot.getProjects()[0]
    assert.notStrictEqual(
      project,
      undefined,
      'Could not open the root TypeScript project'
    )

    const statsStorePath = resolve('app/src/lib/stats/stats-store.ts')
    const sourceFile = project.program.getSourceFile(statsStorePath)
    assert.notStrictEqual(sourceFile, undefined)

    const dailyStatsDeclaration = sourceFile?.statements.find(
      statement =>
        statement.kind === SyntaxKind.TypeAliasDeclaration &&
        isTypeAliasDeclaration(statement) &&
        statement.name.text === 'DailyStats'
    )

    assert.ok(
      dailyStatsDeclaration !== undefined &&
        isTypeAliasDeclaration(dailyStatsDeclaration),
      'Could not find the DailyStats type alias'
    )

    const checker = project.checker
    const type = checker.getTypeAtLocation(dailyStatsDeclaration)
    if (type === undefined) {
      throw new Error('Could not resolve the DailyStats type')
    }
    return { api, project, checker, type }
  } catch (error) {
    api.close()
    throw error
  }
}

function withoutUndefined(type: Type): ReadonlyArray<Type> {
  const types = type.isUnionType() ? (type.getTypes() ?? []) : [type]
  return types.filter(member => (member.flags & TypeFlags.Undefined) === 0)
}

function hasFlag(types: ReadonlyArray<Type>, flag: TypeFlags): boolean {
  return types.some(type => (type.flags & flag) !== 0)
}

function exampleValueForType(type: Type): string | number | boolean | null {
  const types = withoutUndefined(type)

  if (hasFlag(types, TypeFlags.Null)) {
    return null
  }

  if (
    hasFlag(
      types,
      TypeFlags.Number | TypeFlags.NumberLiteral | TypeFlags.NumberLike
    )
  ) {
    return 1
  }

  if (
    hasFlag(
      types,
      TypeFlags.Boolean | TypeFlags.BooleanLiteral | TypeFlags.BooleanLike
    )
  ) {
    return true
  }

  const stringLiteral = types.find(type => type.isStringLiteralType())
  if (stringLiteral !== undefined) {
    return stringLiteral.value
  }

  if (
    hasFlag(
      types,
      TypeFlags.String | TypeFlags.StringLiteral | TypeFlags.StringLike
    )
  ) {
    return 'example'
  }

  throw new Error(`Unsupported stats property type: ${type.flags}`)
}

function buildExampleStatsPayload(): StatsPayload {
  const context = getDailyStatsType()
  try {
    const { checker, project, type } = context
    const entries = checker.getPropertiesOfType(type).map(property => {
      const handle = property.valueDeclaration ?? property.declarations[0]
      const declaration = handle?.resolve(project)
      if (declaration === undefined) {
        throw new Error(`Could not find the declaration for ${property.name}`)
      }

      const propertyType = checker.getTypeOfSymbolAtLocation(
        property,
        declaration
      )
      return [property.name, exampleValueForType(propertyType)]
    })

    return Object.fromEntries(entries) as StatsPayload
  } finally {
    context.api.close()
  }
}

export function generateUsageDataExample(): ITelemetryPayload {
  return buildStatsPayload(buildExampleStatsPayload())
}
