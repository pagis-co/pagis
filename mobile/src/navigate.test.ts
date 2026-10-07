import { describe, expect, it } from 'vitest'

import { placeOf } from './navigate'

/** The stored origin of the server. */
const ORIGIN = 'https://a.example'

/** The rule of the tap on a Notification. `WebOriginTests.swift`
 *  and `ServerOriginTest.java` take the same cases. */
describe('the place that a tap on a Notification opens', () => {
  it('is the path, the query and the fragment of a navigate URL on the stored origin', () => {
    expect(placeOf('https://a.example/c/abc?x=1', ORIGIN)).toBe('/c/abc?x=1')
    expect(placeOf('https://a.example/c/abc#card', ORIGIN)).toBe('/c/abc#card')
    expect(placeOf('https://A.Example:443/c/abc', `${ORIGIN}/`)).toBe('/c/abc')
    expect(placeOf('https://a.example', ORIGIN)).toBe('/')
  })

  it('is the root for a URL of another origin', () => {
    expect(placeOf('https://evil.example/c/abc', ORIGIN)).toBe('/')
    expect(placeOf('http://a.example/c/abc', ORIGIN)).toBe('/')
    expect(placeOf('https://a.example:8443/c/abc', ORIGIN)).toBe('/')
  })

  it('is the root for a value that is not an http URL', () => {
    expect(placeOf('javascript:x', ORIGIN)).toBe('/')
    expect(placeOf('not a url', ORIGIN)).toBe('/')
    expect(placeOf('/c/abc', ORIGIN)).toBe('/')
    expect(placeOf(undefined, ORIGIN)).toBe('/')
    expect(placeOf(7, ORIGIN)).toBe('/')
  })
})
