import { describe, expect, it } from 'vitest'

import { buildSearchToken, getTagSearchURL } from '../../src/lib/tagFormat'

describe('buildSearchToken', () => {
  it('leaves a single-word namespaced value unquoted, with the exact-match suffix when requested', () => {
    expect(buildSearchToken('female', 'milf', true)).toBe('female:milf$')
    expect(buildSearchToken('female', 'milf', false)).toBe('female:milf')
  })

  it('quotes a multi-word value so it survives space being a real token delimiter (issue #59)', () => {
    expect(buildSearchToken('female', 'huge breasts', true)).toBe('"female:huge breasts"')
    // Quoting already implies an exact match server-side — the `$` suffix would be redundant, so
    // `exact` has no separate effect once the value contains a space.
    expect(buildSearchToken('female', 'huge breasts', false)).toBe('"female:huge breasts"')
  })

  it('quotes a bare (non-namespaced) multi-word value the same way', () => {
    expect(buildSearchToken('other', '堕とされたい熟女達', false)).toBe('堕とされたい熟女達')
    expect(buildSearchToken('other', '堕とされたい 熟女達', false)).toBe('"堕とされたい 熟女達"')
  })
})

describe('getTagSearchURL', () => {
  it('builds a plain exact-match URL for a single-word tag', () => {
    expect(getTagSearchURL('female', 'milf')).toBe('/?q=female%3Amilf%24')
  })

  it('quotes a multi-word tag value instead of leaving a bare space in the query', () => {
    expect(getTagSearchURL('female', 'huge breasts')).toBe('/?q=%22female%3Ahuge%20breasts%22')
  })

  it('leaves source tags as external links, untouched by quoting', () => {
    expect(getTagSearchURL('source', 'e-hentai.org/g/123/abc')).toBe('https://e-hentai.org/g/123/abc')
  })
})
