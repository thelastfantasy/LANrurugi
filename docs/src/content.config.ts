import { defineCollection } from 'astro:content'
import { docsLoader, i18nLoader } from '@astrojs/starlight/loaders'
import { docsSchema, i18nSchema } from '@astrojs/starlight/schema'

// Starlight's content-layer setup. The `i18n` collection is declared even though no override file
// exists yet: Starlight's own UI strings already cover Simplified Chinese, and omitting the
// collection entirely makes every build log "collection i18n does not exist". Declaring it is also
// what makes adding a UI-string override later a file drop rather than a config change.
export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
  i18n: defineCollection({ loader: i18nLoader(), schema: i18nSchema() }),
}
