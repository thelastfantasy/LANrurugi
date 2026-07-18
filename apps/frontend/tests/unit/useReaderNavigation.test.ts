import { describe, expect, it } from 'vitest'

import { clamp, computeNextPage, computeSpread } from '../../src/pages/Reader/useReaderNavigation'

describe('computeNextPage', () => {
  it('advances forward on next, clamped to totalPages', () => {
    expect(computeNextPage('next', 5, 10, false, false, false)).toBe(6)
    expect(computeNextPage('next', 10, 10, false, false, false)).toBe(10)
  })

  it('goes back on prev, clamped to 1', () => {
    expect(computeNextPage('prev', 5, 10, false, false, false)).toBe(4)
    expect(computeNextPage('prev', 1, 10, false, false, false)).toBe(1)
  })

  it('negates the offset in manga mode (left reads forward)', () => {
    expect(computeNextPage('next', 5, 10, true, false, false)).toBe(4)
    expect(computeNextPage('prev', 5, 10, true, false, false)).toBe(6)
  })

  it('steps by 2 in double-page mode while a spread is showing', () => {
    expect(computeNextPage('next', 5, 10, false, true, true)).toBe(7)
    expect(computeNextPage('prev', 5, 10, false, true, true)).toBe(3)
  })

  it('steps by 1 in double-page mode when not currently showing a spread (edge page)', () => {
    expect(computeNextPage('next', 1, 10, false, true, false)).toBe(2)
  })

  it('first/last targets respect manga-mode direction swap', () => {
    expect(computeNextPage('first', 5, 10, false, false, false)).toBe(1)
    expect(computeNextPage('last', 5, 10, false, false, false)).toBe(10)
    expect(computeNextPage('first', 5, 10, true, false, false)).toBe(10)
    expect(computeNextPage('last', 5, 10, true, false, false)).toBe(1)
  })
})

describe('clamp', () => {
  it('clamps within bounds', () => {
    expect(clamp(5, 1, 10)).toBe(5)
    expect(clamp(0, 1, 10)).toBe(1)
    expect(clamp(11, 1, 10)).toBe(10)
  })
})

describe('computeSpread', () => {
  it('shows a single page at either edge of the archive', () => {
    expect(computeSpread(1, 10, true, false, () => false)).toEqual({ left: 1, right: null })
    expect(computeSpread(10, 10, true, false, () => false)).toEqual({ left: 10, right: null })
  })

  it('shows a single page when double-page mode is off', () => {
    expect(computeSpread(5, 10, false, false, () => false)).toEqual({ left: 5, right: null })
  })

  it('pairs current+partner in LTR double-page mode', () => {
    expect(computeSpread(5, 10, true, false, () => false)).toEqual({ left: 5, right: 6 })
  })

  it('swaps left/right in manga (RTL) double-page mode so the higher page reads first', () => {
    expect(computeSpread(5, 10, true, true, () => false)).toEqual({ left: 6, right: 5 })
  })

  it('falls back to a single page when either page in the pair is a widespread', () => {
    expect(computeSpread(5, 10, true, false, (page) => page === 5)).toEqual({ left: 5, right: null })
    expect(computeSpread(5, 10, true, false, (page) => page === 6)).toEqual({ left: 5, right: null })
  })

  it('falls back to a single page when the partner would exceed totalPages', () => {
    expect(computeSpread(10, 10, true, false, () => false)).toEqual({ left: 10, right: null })
  })
})
