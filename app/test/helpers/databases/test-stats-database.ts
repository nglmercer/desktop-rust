import { StatsDatabase } from '../../../src/lib/stats'

export class TestStatsDatabase extends StatsDatabase {
  public constructor(name = 'TestStatsDatabase') {
    super(name)
  }

  public async reset(): Promise<void> {
    await this.delete()
    await this.open()
  }
}
