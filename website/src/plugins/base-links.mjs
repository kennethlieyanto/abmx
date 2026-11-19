// Prefix root-relative links in rendered Markdown with Astro's `base`.
//
// GitHub Pages serves this site under /abmx, and Astro does not rewrite
// author-written Markdown links. Without this, every internal link in the docs
// would silently point at the user site root.

import { defineHastPlugin } from 'satteri';

const EXTERNAL = /^[a-z][a-z0-9+.-]*:/i;

/** @param {string} url @param {string} base */
function withBase(url, base) {
  if (!url.startsWith('/') || url.startsWith('//')) return url;
  if (base === '/') return url;
  if (url === base || url.startsWith(`${base}/`)) return url;
  if (url === '/') return `${base}/`;
  return `${base}${url}`;
}

/**
 * @param {string} rawBase
 * @param {Record<string, unknown>} properties
 * @param {string} attribute
 */
function rewrite(properties, attribute, base) {
  const value = properties?.[attribute];
  if (typeof value === 'string' && !EXTERNAL.test(value)) {
    return withBase(value, base);
  }
  return undefined;
}

/** @param {string} [rawBase] */
export default function baseLinks(rawBase = '/') {
  const base = rawBase.length > 1 ? rawBase.replace(/\/+$/, '') : '/';

  return defineHastPlugin({
    name: 'abmx-base-links',
    element: [
      {
        filter: ['a'],
        visit(node, ctx) {
          const href = rewrite(node.properties, 'href', base);
          if (href !== undefined) ctx.setProperty(node, 'href', href);
        },
      },
      {
        filter: ['img'],
        visit(node, ctx) {
          const src = rewrite(node.properties, 'src', base);
          if (src !== undefined) ctx.setProperty(node, 'src', src);
        },
      },
    ],
  });
}
