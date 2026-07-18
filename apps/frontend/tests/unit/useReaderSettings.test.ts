import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import { useReaderSettings } from '../../src/pages/Reader/useReaderSettings'

describe('useReaderSettings', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('reads defaults when localStorage is empty', () => {
    const { result } = renderHook(() => useReaderSettings())
    const [settings] = result.current
    expect(settings.hideHeader).toBe(false)
    expect(settings.mangaMode).toBe(false)
    expect(settings.fitMode).toBe('container')
    expect(settings.preloadCount).toBe(2)
    expect(settings.autoNextPageInterval).toBe(10)
  })

  it('persists a boolean field round-trip through localStorage', () => {
    const { result } = renderHook(() => useReaderSettings())

    act(() => {
      const [, update] = result.current
      update({ mangaMode: true })
    })

    expect(localStorage.getItem('mangaMode')).toBe('true')
    const [settings] = result.current
    expect(settings.mangaMode).toBe(true)

    const { result: reloaded } = renderHook(() => useReaderSettings())
    expect(reloaded.current[0].mangaMode).toBe(true)
  })

  it('removes the fitMode key when set back to container (matches legacy: unset = container)', () => {
    const { result } = renderHook(() => useReaderSettings())

    act(() => {
      const [, update] = result.current
      update({ fitMode: 'fit-width' })
    })
    expect(localStorage.getItem('fitMode')).toBe('fit-width')

    act(() => {
      const [, update] = result.current
      update({ fitMode: 'container' })
    })
    expect(localStorage.getItem('fitMode')).toBeNull()
  })

  it('persists a numeric field as a string and reads it back as a number', () => {
    const { result } = renderHook(() => useReaderSettings())

    act(() => {
      const [, update] = result.current
      update({ preloadCount: 5 })
    })
    expect(localStorage.getItem('preloadCount')).toBe('5')

    const { result: reloaded } = renderHook(() => useReaderSettings())
    expect(reloaded.current[0].preloadCount).toBe(5)
  })
})
