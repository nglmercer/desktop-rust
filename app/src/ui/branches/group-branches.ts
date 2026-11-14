import type { Branch } from '../../models/branch'
import type { IFilterListGroup, IFilterListItem } from '../lib/filter-list'

export type BranchGroupIdentifier = 'default' | 'recent' | 'other'

export interface IBranchListItem extends IFilterListItem {
  readonly text: ReadonlyArray<string>
  readonly id: string
  readonly branch: Branch
}

export function groupBranches(
  defaultBranch: Branch | null,
  _currentBranch: Branch | null,
  allBranches: ReadonlyArray<Branch>,
  recentBranches: ReadonlyArray<Branch>
): ReadonlyArray<IFilterListGroup<IBranchListItem>> {
  const groups: IFilterListGroup<IBranchListItem>[] = []

  if (defaultBranch) {
    groups.push({
      identifier: 'default',
      items: [
        {
          text: [defaultBranch.name],
          id: defaultBranch.name,
          branch: defaultBranch,
        },
      ],
    })
  }

  const recentBranchNames = new Set<string>()
  const defaultBranchName = defaultBranch ? defaultBranch.name : null
  const recentBranchesWithoutDefault = recentBranches.filter(
    b => b.name !== defaultBranchName
  )
  if (recentBranchesWithoutDefault.length > 0) {
    const recentBranches: IBranchListItem[] = []

    for (const branch of recentBranchesWithoutDefault) {
      recentBranches.push({
        text: [branch.name],
        id: branch.name,
        branch,
      })
      recentBranchNames.add(branch.name)
    }

    groups.push({
      identifier: 'recent',
      items: recentBranches,
    })
  }

  const remainingBranches = allBranches.filter(
    b =>
      b.name !== defaultBranchName &&
      !recentBranchNames.has(b.name) &&
      !b.isDesktopForkRemoteBranch
  )

  const remainingItems = remainingBranches.map(b => ({
    text: [b.name],
    id: b.name,
    branch: b,
  }))
  groups.push({
    identifier: 'other',
    items: remainingItems,
  })

  return groups
}
