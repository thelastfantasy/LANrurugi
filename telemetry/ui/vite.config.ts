import { defineConfig } from "vite"
import { svelte } from "@sveltejs/vite-plugin-svelte"

// One page, no router: the server hands this bundle to anyone who asks for `/`, and the app
// decides between the login form and the dashboard from `/api/session`.
export default defineConfig({
  plugins: [svelte()],
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
})
