//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A minimal LRU-capped Map for module-level webview stores that live for the
// whole Tauri session (it never reloads) and would otherwise grow by one
// small entry per distinct key — day, task, app-icon lookup, … — for as long
// as the app runs. Realistic usage cardinality keeps these well under any
// sane cap in practice; this is a hygiene bound, not a fix for an observed
// runaway.
//
// Deliberately NOT a full Map re-implementation — just the get/set subset the
// module-level stores that use this actually need (see planStore.ts,
// useWorklog.ts, app-icons.ts). Recency is tracked by re-inserting the
// touched key so the underlying Map's insertion order IS recency order (a
// well-known JS idiom: Map iterates in insertion order, and re-`set`ting an
// existing key does NOT move it, so the delete-then-set below is what
// actually bumps it to "most recent").

export class LruMap<K, V> {
  private readonly map = new Map<K, V>()

  constructor(private readonly capacity: number) {
    // Reject NaN/Infinity/fractional too: `NaN < 1` and `Infinity < 1` are both
    // false, so a bare `< 1` check would accept them and then `size > capacity`
    // is never true — the bound would silently never evict.
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new Error('LruMap capacity must be an integer >= 1')
    }
  }

  /** Reads `key`, marking it most-recently-used. `undefined` means "not
   *  cached" — a value of `undefined` is never itself stored by any current
   *  caller, so this is unambiguous for them. */
  get(key: K): V | undefined {
    if (!this.map.has(key)) return undefined
    const value = this.map.get(key) as V
    // Bump recency: delete + re-set moves the key to the end of iteration
    // order (Map's order is insertion order; re-setting an existing key
    // alone would NOT move it).
    this.map.delete(key)
    this.map.set(key, value)
    return value
  }

  /** Writes `key`, marking it most-recently-used, and evicts the single
   *  least-recently-used entry if this insert pushed the map past capacity. */
  set(key: K, value: V): void {
    this.map.delete(key)
    this.map.set(key, value)
    if (this.map.size > this.capacity) {
      // Use the iterator's `done` flag, not `value !== undefined`: an explicit
      // `undefined` key is a valid Map entry and would otherwise never evict
      // (it's indistinguishable from the empty-iterator sentinel).
      const oldest = this.map.keys().next()
      if (!oldest.done) {
        this.map.delete(oldest.value)
      }
    }
  }

  has(key: K): boolean {
    return this.map.has(key)
  }

  delete(key: K): boolean {
    return this.map.delete(key)
  }

  get size(): number {
    return this.map.size
  }
}
