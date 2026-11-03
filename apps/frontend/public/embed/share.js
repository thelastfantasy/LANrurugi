/*
 * <lrrg-share code="XXXXX-XXXXX" src="https://library.example.org" theme="ex"></lrrg-share>
 *
 * Renders a LANrurugi share card by framing the sharing instance's own card page, so the element
 * and the iframe snippet always show the same card. The card reports its height so the frame fits
 * its content; nothing else crosses between the page and the card.
 *
 * `theme` is optional; without it the card uses the sharing library's own theme. It is a preset
 * (system, light, dark, g, ex, modern, modern_red, modern_clear) or colours over one, as JSON in
 * the attribute or as an object on the property:
 *
 *   el.theme = { base: "ex", bg: "#101820", fg: "#f5f5f5", muted: "#99a", border: "#333",
 *                accent: "#f2aa4c", tile: "#222", qrFg: "#101820", qrBg: "#f5f5f5", qrFinder: "#f2aa4c" }
 *
 * Every key is optional, and only valid CSS colours are used.
 *
 * The card's text follows the embedding page's language (`lang` on the element, an ancestor or
 * <html>), in Chinese, English or Japanese.
 */
(function () {
  if (window.customElements.get("lrrg-share")) return

  class LrrgShare extends HTMLElement {
    static get observedAttributes() {
      return ["code", "src", "theme", "lang"]
    }

    /** The `theme` attribute: a preset name, or an object kept as JSON. */
    get theme() {
      const value = this.getAttribute("theme")
      if (value && value.trim().charAt(0) === "{") {
        try {
          return JSON.parse(value)
        } catch {
          return value
        }
      }
      return value
    }

    set theme(value) {
      if (value == null || value === "") this.removeAttribute("theme")
      else this.setAttribute("theme", typeof value === "string" ? value : JSON.stringify(value))
    }

    connectedCallback() {
      // A page may set `el.theme = {…}` before this script defines the element; that lands as a
      // plain property hiding the setter. Re-apply it through the setter (which renders).
      if (Object.prototype.hasOwnProperty.call(this, "theme")) {
        const value = this.theme
        delete this.theme
        this.theme = value
        if (this.getAttribute("theme")) return
      }
      this.render()
    }

    disconnectedCallback() {
      this.stopListening()
    }

    attributeChangedCallback() {
      if (this.isConnected) this.render()
    }

    stopListening() {
      if (this.onMessage) window.removeEventListener("message", this.onMessage)
      this.onMessage = null
    }

    render() {
      this.stopListening()
      const code = (this.getAttribute("code") || "").trim()
      const src = (this.getAttribute("src") || "").trim().replace(/\/+$/, "")
      const root = this.shadowRoot || this.attachShadow({ mode: "open" })
      root.textContent = ""
      if (!code || !/^https?:\/\//.test(src)) return
      const frame = document.createElement("iframe")
      const theme = (this.getAttribute("theme") || "").trim()
      // The card speaks the embedding page's language: this element's `lang`, else the nearest
      // ancestor's, else the page's own.
      const scope = this.closest("[lang]")
      const lang = ((scope && scope.getAttribute("lang")) || document.documentElement.lang || "").trim()
      // `fit`: this frame takes the card's height, so the card page shows no scrollbar of its own.
      frame.src =
        src +
        "/embed/s/" +
        encodeURIComponent(code) +
        "?fit=1" +
        (theme ? "&theme=" + encodeURIComponent(theme) : "") +
        (lang ? "&lang=" + encodeURIComponent(lang) : "")
      frame.title = "LANrurugi share"
      frame.loading = "lazy"
      frame.referrerPolicy = "no-referrer"
      frame.style.cssText = "border:0;width:100%;max-width:480px;min-width:260px;height:360px;display:block"
      const origin = new URL(src).origin
      this.onMessage = function (event) {
        if (event.origin !== origin || event.source !== frame.contentWindow) return
        const data = event.data
        if (data && data.type === "lrrg-share-height" && typeof data.height === "number") {
          frame.style.height = Math.min(Math.max(data.height, 120), 1200) + "px"
        }
      }
      window.addEventListener("message", this.onMessage)
      root.appendChild(frame)
    }
  }

  window.customElements.define("lrrg-share", LrrgShare)
})()
