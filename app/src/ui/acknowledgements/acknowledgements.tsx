import * as Path from 'path'
import * as Fs from 'fs'
import * as React from 'react'
import { getAppPath } from '../lib/app-proxy'
import { Loading } from '../lib/loading'
import { LinkButton } from '../lib/link-button'
import { Dialog, DialogContent, DefaultDialogFooter } from '../dialog'

const WebsiteURL = 'https://desktop.github.com'
const RepositoryURL = 'https://github.com/desktop/desktop'

interface IAcknowledgementsProps {
  /** The function to call when the dialog should be dismissed. */
  readonly onDismissed: () => void

  /**
   * The currently installed (and running) version of the app.
   */
  readonly applicationVersion: string
}

interface ILicense {
  readonly repository?: string
  readonly sourceText?: string
  readonly license?: string
}

type Licenses = { [key: string]: ILicense }

function isLicenses(value: unknown): value is Licenses {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return false
  }
  return Object.values(value).every(entry => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) {
      return false
    }
    return ['repository', 'sourceText', 'license'].every(key => {
      const field = (entry as Record<string, unknown>)[key]
      return field === undefined || typeof field === 'string'
    })
  })
}

interface IAcknowledgementsState {
  readonly licenses: Licenses | null
  readonly failed: boolean
}

/** The component which displays the licenses for packages used in the app. */
export class Acknowledgements extends React.Component<
  IAcknowledgementsProps,
  IAcknowledgementsState
> {
  public constructor(props: IAcknowledgementsProps) {
    super(props)

    this.state = { licenses: null, failed: false }
  }

  public async componentDidMount() {
    let appPath: string
    try {
      appPath = await getAppPath()
    } catch (error) {
      log.error('Error locating licenses', error)
      this.setState({ failed: true })
      return
    }
    const path = Path.join(appPath, 'static', 'licenses.json')
    Fs.readFile(path, 'utf8', (err, data) => {
      if (err) {
        log.error('Error loading licenses', err)
        this.setState({ failed: true })
        return
      }

      try {
        const parsed = JSON.parse(data)
        if (!isLicenses(parsed)) {
          throw new Error('Invalid license list')
        }
        this.setState({ licenses: parsed })
      } catch (error) {
        log.error('Error parsing licenses', error)
        this.setState({ failed: true })
      }
    })
  }

  private renderLicenses(licenses: Licenses) {
    const elements = []
    for (const key of Object.keys(licenses).sort()) {
      // Desktop's own notice is shown separately above the dependency list.
      if (key === `desktop@${this.props.applicationVersion}`) {
        continue
      }

      const license = licenses[key]
      const repository = license.repository
      let nameElement

      if (repository) {
        const uri = normalizedGitHubURL(repository)
        nameElement = <LinkButton uri={uri}>{key}</LinkButton>
      } else {
        nameElement = key
      }

      let licenseText

      if (license.sourceText) {
        licenseText = license.sourceText
      } else if (license.license) {
        licenseText = `License: ${license.license}`
      } else {
        licenseText = 'Unknown license'
      }

      const nameHeader = <h2 key={`${key}-header`}>{nameElement}</h2>
      const licenseParagraph = (
        <p key={`${key}-text`} className="license-text">
          {licenseText}
        </p>
      )

      elements.push(nameHeader, licenseParagraph)
    }

    return elements
  }

  public render() {
    const licenses = this.state.licenses

    let desktopLicense: JSX.Element | null = null
    if (licenses) {
      const key = `desktop@${this.props.applicationVersion}`
      const entry = licenses[key]
      if (entry?.sourceText) {
        desktopLicense = <p className="license-text">{entry.sourceText}</p>
      }
    }

    return (
      <Dialog
        id="acknowledgements"
        title="License and Open Source Notices"
        onSubmit={this.props.onDismissed}
        onDismissed={this.props.onDismissed}
      >
        <DialogContent>
          <p>
            <LinkButton uri={WebsiteURL}>GitHub Desktop</LinkButton> is an open
            source project published under the MIT License. You can view the
            source code and contribute to this project on{' '}
            <LinkButton uri={RepositoryURL}>GitHub</LinkButton>.
          </p>

          {desktopLicense}

          <p>GitHub Desktop also distributes these libraries:</p>

          {licenses ? (
            this.renderLicenses(licenses)
          ) : this.state.failed ? (
            <p>License notices could not be loaded.</p>
          ) : (
            <Loading />
          )}
        </DialogContent>

        <DefaultDialogFooter />
      </Dialog>
    )
  }
}

/** Normalize a package URL to a GitHub URL. */
function normalizedGitHubURL(url: string): string {
  let newURL = url
  newURL = newURL.replace('git+https://github.com', 'https://github.com')
  newURL = newURL.replace('git+ssh://git@github.com', 'https://github.com')
  return newURL
}
