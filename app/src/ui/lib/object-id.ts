import { randomUUID } from '../../web/random-uuid'

const idMap = new WeakMap<object, string>()

/**
 * Generates a unique ID for the given object reference.
 *
 * The same (by reference equality) object will get the same id for the lifetime
 * of the application. This is achieved by using a weak reference to the object.
 *
 */
export function getObjectId(obj: object): string {
  let id = idMap.get(obj)

  if (id === undefined) {
    id = randomUUID()
    idMap.set(obj, id)
  }

  return id
}
