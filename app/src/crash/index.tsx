import * as React from 'react'
import * as ReactDOM from 'react-dom'

import { CrashApp } from './crash-app'

import './styles/crash.scss'

document.body.classList.add(`platform-${process.platform}`)

const container = document.createElement('div')
container.id = 'desktop-crash-container'
document.body.appendChild(container)

ReactDOM.render(<CrashApp />, container)
