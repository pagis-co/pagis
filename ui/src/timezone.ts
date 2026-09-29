// The timezone of this browser or of the Client App window. The daemon
// takes it at a Person's first sign-in, so their Schedules run on their
// own clock and not on the server's.

/** This device's IANA timezone, or `undefined` when the runtime names
 *  none. */
export function deviceTimezone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined
  } catch {
    return undefined
  }
}

/** Every IANA timezone this runtime knows, with `current` in it. */
export function knownTimezones(current: string): string[] {
  const known = Intl.supportedValuesOf('timeZone')
  return known.includes(current) ? known : [current, ...known]
}
