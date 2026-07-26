// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import tailwind from '@tailwindcss/vite';
import starlightThemeBlack from 'starlight-theme-black'

// https://astro.build/config
export default defineConfig({
	// The typing pages moved under /typing/; published links keep working.
	redirects: {
		'/typing': '/typing/guide',
		'/typing-reference': '/typing/reference',
	},
	integrations: [
		starlight({
			title: 'Roughly',
			logo: {
				src: './public/logo.svg',
				replacesTitle: true,
			},
			favicon: '/favicon.svg',
			social: [
				{ label: "Visual Studio Marketplace", icon: "vscode", href: 'https://marketplace.visualstudio.com/items?itemName=felix-andreas.roughly' },
				{ label: "GitHub", icon: "github", href: 'https://github.com/felix-andreas/roughly' },
			],
			sidebar: [
				{
					label: 'Roughly',
					items: [
						{ slug: 'getting-started' },
						{ slug: 'installation' },
						{ slug: 'formatter' },
						{ slug: 'linter' },
						{ slug: 'language-server' },
						{ slug: 'configuration' },
						{ slug: 'diagnostics' },
						{ slug: 'limitations' },
					],
				},
				{
					label: 'Typing',
					items: [
						{ slug: 'typing/guide' },
						{ slug: 'typing/reference' },
						{ slug: 'stdlib-stubs' },
					],
				},
				{
					label: 'Contributing',
					items: [
						{ slug: 'development' },
						{ slug: 'architecture' },
						{ slug: 'structure' },
						{ slug: 'testing' },
					],
				},
			],
			customCss: ['./src/tailwind.css'],
			plugins: [
				starlightThemeBlack({
					navLinks: [
						{
							label: 'Docs',
							link: '/getting-started',
						},
						{
							label: 'Formatter',
							link: '/formatter',
						},
						{
							label: 'Typing',
							link: '/typing/guide',
						},
						{
							label: 'Linter',
							link: '/linter',
						},
						{
							label: 'Config',
							link: '/configuration',
						},
					],
					footerText: `<div class="py-8 flex items-center justify-between"><div class="flex items-center gap-2"><img src="/logo.svg" width="12" /> Roughly © ${new Date().getFullYear()}</div><a href="https://felixandreas.me/legal-notice/" target="_blank" rel="noopener" class="no-underline text-gray-500">Legal Notice</a></div>`
				})
			],
		}),
	],
	vite: {
		plugins: [tailwind()],
	},
});
