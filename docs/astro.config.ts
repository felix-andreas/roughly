// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import tailwind from '@astrojs/tailwind';
import starlightThemeBlack from 'starlight-theme-black'

// https://astro.build/config
export default defineConfig({
	integrations: [
		starlight({
			title: 'Roughly',
			logo: {
				src: './public/logo.svg',
				replacesTitle: true,
			},
			favicon: '/favicon.svg',
			social: {
				github: 'https://github.com/felix-andreas/roughly',
			},
			sidebar: [
				{
					label: 'Roughly',
					items: [
						{ slug: 'getting-started' },
						{ slug: 'formatter' },
						{ slug: 'linter' },
						{ slug: 'language-server' },
						{ slug: 'configuration' },
					],
				},
			],
			customCss: ['./src/tailwind.css'],
			plugins: [
				starlightThemeBlack({
					navLinks: [
						{
							label: 'Formatter',
							link: '/formatter',
						},
						{
							label: 'Linter',
							link: '/linter',
						},
						{
							label: 'Configuration',
							link: '/configuration',
						},
					],
					footerText: `<div class="py-8 flex items-center justify-between"><div class="flex items-center gap-2"><img src="logo.svg" width="12" /> Roughly © ${new Date().getFullYear()}</div><a href="https://felixandreas.me/legal-notice/" target="_blank" rel="noopener" class="no-underline text-gray-500">Legal Notice</a></div>`
				})
			],
		}),
		tailwind({ applyBaseStyles: false }),
	],
});
