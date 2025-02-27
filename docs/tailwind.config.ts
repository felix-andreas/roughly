import colors from 'tailwindcss/colors';
import starlightPlugin from '@astrojs/starlight-tailwind';

/** @type {import('tailwindcss').Config} */
export default {
	content: ['./src/**/*.{astro,html,js,jsx,md,mdx,svelte,ts,tsx,vue}', 'astro.config.ts'],
	theme: {
		extend: {
			colors: {
				accent: colors.gray,
				gray: colors.gray,
			},
			fontFamily: {
				sans: ['"Inter"'],
				mono: ['"JetBrains Mono"'],
			},
		},
	},
	plugins: [starlightPlugin()],
}
