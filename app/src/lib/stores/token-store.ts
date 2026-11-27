import {
  deleteHostSecret,
  getHostSecret,
  setHostSecret,
} from '../host-secret-store'

function setItem(key: string, login: string, value: string) {
  return setHostSecret(key, login, value)
}

function getItem(key: string, login: string) {
  return getHostSecret(key, login)
}

function deleteItem(key: string, login: string) {
  return deleteHostSecret(key, login)
}

export const TokenStore = {
  setItem,
  getItem,
  deleteItem,
}
