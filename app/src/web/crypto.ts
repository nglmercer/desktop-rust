// `crypto` for the webview build: the browser implementation of Node's crypto
// module, plus `randomUUID`. That implementation has no `randomUUID`, and the
// Copilot SDK imports it (`import { randomUUID } from "node:crypto"`).

import { randomUUID } from './random-uuid'
import './install-random-uuid'
// @ts-expect-error -- aliased by desktop-crypto-impl in vite.config.ts
import cryptoBrowserify from 'desktop-crypto-impl'

export { randomUUID }

// Vite's development optimizer exposes this CommonJS dependency as a default
// export. Re-export the named APIs used by the renderer explicitly.
export const randomBytes: typeof import('crypto').randomBytes =
  cryptoBrowserify.randomBytes
export const createHash: typeof import('crypto').createHash =
  cryptoBrowserify.createHash
export const {
  Cipher,
  Cipheriv,
  Decipher,
  Decipheriv,
  DiffieHellman,
  DiffieHellmanGroup,
  Hash,
  Hmac,
  Sign,
  Verify,
  constants,
  createCipher,
  createCipheriv,
  createCredentials,
  createDecipher,
  createDecipheriv,
  createDiffieHellman,
  createDiffieHellmanGroup,
  createECDH,
  createHmac,
  createSign,
  createVerify,
  getCiphers,
  getDiffieHellman,
  getHashes,
  listCiphers,
  pbkdf2,
  pbkdf2Sync,
  privateDecrypt,
  privateEncrypt,
  prng,
  pseudoRandomBytes,
  publicDecrypt,
  publicEncrypt,
  randomFill,
  randomFillSync,
  rng,
} = cryptoBrowserify
