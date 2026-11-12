import type { StatsPayload } from './stats-store'

interface ITelemetryEvent {
  readonly app: 'desktop'
  readonly event_type: 'usage' | 'ping'
  readonly dimensions: Readonly<Record<string, string>>
  readonly measures?: Readonly<Record<string, number>>
}

interface ITelemetryPayload {
  readonly events: ReadonlyArray<ITelemetryEvent>
}

function stringifyDimensions<
  T extends { readonly [K in keyof T]: string | boolean | null },
>(dimensions: T): Readonly<Record<string, string>> {
  return Object.fromEntries(
    Object.entries(dimensions).map(([key, value]) => [key, String(value)])
  )
}

/**
 * Transform the flat payload accepted by Central into CAFE's TelemetryAPI
 * event format. CAFE expects `eventType` as `event_type`, dimensions encoded
 * as strings, and measures encoded as integers inside an `events` array.
 * Central previously performed the string conversion and integer rounding
 * server-side, so the CAFE payload must perform those conversions here.
 *
 * This conversion stays at the HTTP boundary so the legacy Central path can
 * continue sending the original payload unchanged.
 */
export function buildStatsPayload(body: StatsPayload): ITelemetryPayload {
  if (body.eventType === 'ping') {
    return {
      events: [
        {
          app: 'desktop',
          event_type: body.eventType,
          dimensions: stringifyDimensions({
            optIn: body.optIn,
            previousOptInValue: body.previousOptInValue,
          }),
        },
      ],
    }
  }

  const {
    eventType,
    version,
    osVersion,
    platform,
    architecture,
    guid,
    theme,
    selectedTerminalEmulator,
    selectedTextEditor,
    diffMode,
    dotComAccount,
    enterpriseAccount,
    usesShortLivedToken,
    notificationsEnabled,
    notificationsPermission,
    launchedFromApplicationsFolder,
    linkUnderlinesVisible,
    diffCheckMarksVisible,
    useExternalCredentialHelper,
    filteringChangesEnabled,
    gitHooksEnvEnabled,
    copilotConflictResolutionModel,
    active,
    tutorialStarted,
    tutorialRepoCreated,
    tutorialEditorInstalled,
    tutorialBranchCreated,
    tutorialFileEdited,
    tutorialCommitCreated,
    tutorialBranchPushed,
    tutorialPrCreated,
    tutorialCompleted,
    mainReadyTime,
    loadTime,
    rendererReadyTime,
    ...remainingMeasures
  } = body

  // Central converted dimension values to strings for us.
  const dimensions = stringifyDimensions({
    version,
    osVersion,
    platform,
    architecture,
    guid,
    theme,
    selectedTerminalEmulator,
    selectedTextEditor,
    diffMode,
    dotComAccount,
    enterpriseAccount,
    usesShortLivedToken,
    notificationsEnabled,
    notificationsPermission,
    launchedFromApplicationsFolder,
    linkUnderlinesVisible,
    diffCheckMarksVisible,
    useExternalCredentialHelper: useExternalCredentialHelper ?? null,
    filteringChangesEnabled,
    gitHooksEnvEnabled,
    copilotConflictResolutionModel,
    active,
    tutorialStarted,
    tutorialRepoCreated,
    tutorialEditorInstalled,
    tutorialBranchCreated,
    tutorialFileEdited,
    tutorialCommitCreated,
    tutorialBranchPushed,
    tutorialPrCreated,
    tutorialCompleted,
  })

  // Central rounded decimal measures to integers for us.
  const measures = {
    ...remainingMeasures,
    mainReadyTime: Math.round(mainReadyTime),
    loadTime: Math.round(loadTime),
    rendererReadyTime: Math.round(rendererReadyTime),
  }

  return {
    events: [
      {
        app: 'desktop',
        event_type: eventType,
        dimensions,
        measures,
      },
    ],
  }
}
