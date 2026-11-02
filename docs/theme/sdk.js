// E-H / EX palette switch for the deno doc pages (injected by scripts/theme-sdk.mjs). It drives the
// same `html.dark` class and `theme` storage key as deno doc's own toggle, which stays in the page
// hidden because deno doc's script looks it up on load.
;(() => {
  const root = document.documentElement
  const sw = document.querySelector('.lr-theme-switch')
  if (!sw) return
  const buttons = sw.querySelectorAll('button')

  const sync = () => {
    const dark = root.classList.contains('dark')
    for (const b of buttons) b.setAttribute('aria-pressed', String((b.dataset.theme === 'ex') === dark))
  }

  for (const b of buttons) {
    b.addEventListener('click', () => {
      const dark = b.dataset.theme === 'ex'
      root.classList.toggle('dark', dark)
      root.classList.toggle('light', !dark)
      try {
        localStorage.setItem('theme', dark ? 'dark' : 'light')
      } catch {
        // Storage can be unavailable (private mode); the choice then lasts for this page only.
      }
      sync()
    })
  }

  sync()
  // deno doc settles the theme in its own load handler; the pill only animates after that.
  window.addEventListener('load', () => {
    sync()
    requestAnimationFrame(() => sw.classList.add('ready'))
  })

  // deno doc's search placeholders ("Click or press 'S' to search...") are cut off in the narrow
  // compact (<800px) box. Its own load/focus/blur handlers set them; these run after each and shorten.
  const search = document.getElementById('searchbar')
  const narrow = window.matchMedia('(width < 800px)')
  const shorten = () => {
    if (search && narrow.matches) search.placeholder = 'Search'
  }
  window.addEventListener('load', () => {
    shorten()
    search?.addEventListener('focus', shorten)
    search?.addEventListener('blur', shorten)
  })
})()
