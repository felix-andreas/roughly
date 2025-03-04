import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { docsSchema } from '@astrojs/starlight/schema';
import { ExtendDocsSchema } from 'starlight-theme-black/schema'


export const collections = {
	docs: defineCollection({
		loader: glob({
			base: "./content",
			pattern: `**/[^_]*.{${["md", "mdx"].join(',')}}`,
		}),
		schema: docsSchema({
			extend: ExtendDocsSchema,
		}),
	}),
}