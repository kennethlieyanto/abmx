import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import sitemap from '@astrojs/sitemap';
import { satteri } from '@astrojs/markdown-satteri';
import baseLinks from './src/plugins/base-links.mjs';

// GitHub Pages project site. Everything below assumes `base` stays in sync with
// the repository name; the HAST plugin prefixes root-relative Markdown links.
const base = '/abmx';

export default defineConfig({
  site: 'https://kennethlieyanto.github.io',
  base,
  trailingSlash: 'always',
  markdown: {
    processor: satteri({
      hastPlugins: [baseLinks(base)],
    }),
  },
  integrations: [
    starlight({
      title: 'abmx',
      description: 'Sync bank transaction emails into an Actual Budget ledger.',
      favicon: '/favicon.svg',
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/kennethlieyanto/abmx',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/kennethlieyanto/abmx/edit/main/website/',
      },
      lastUpdated: true,
      // Pagefind's WASM build needs CPU features some older x86_64 hosts lack.
      // Local builds can skip it; CI always builds the index.
      pagefind: !process.env.ABMX_NO_PAGEFIND,
      customCss: ['./src/styles/custom.css'],
      plugins: [starlightLinksValidator()],
      sidebar: [
        {
          label: 'Start here',
          items: [
            'start/introduction',
            'start/install',
            'start/quickstart',
            'start/setup',
            'start/configuration',
            'start/cli',
          ],
        },
        {
          label: 'Guides',
          items: [
            'guides/gmail',
            'guides/actual',
            'guides/statements',
            {
              label: 'Authoring',
              items: [
                'guides/authoring',
                'guides/authoring-receipts',
                'guides/authoring-statements',
              ],
            },
            {
              label: 'Deployment',
              collapsed: true,
              items: [
                'guides/deployment/nixos',
                'guides/deployment/kubernetes',
                'guides/deployment/docker',
              ],
            },
          ],
        },
        {
          label: 'Reference',
          items: [
            'reference/parser-spec',
            'reference/statement-spec',
            'reference/transaction',
            'reference/architecture',
            'reference/security',
          ],
        },
      ],
    }),
    sitemap(),
  ],
});
