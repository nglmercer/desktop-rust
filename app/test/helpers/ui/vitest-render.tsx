import './setup-environment'
import { afterEach } from 'vitest'
import { cleanup } from '@testing-library/react'

afterEach(() => cleanup())

export { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
