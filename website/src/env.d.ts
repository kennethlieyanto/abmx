/// <reference types="astro/client" />

// Starlight exposes its resolved config and the currently selected component
// implementations as Vite virtual modules. Starlight does not ship typings for
// them, so declare the ones this project's overrides import.

declare module 'virtual:starlight/user-config' {
  import type { StarlightConfig } from '@astrojs/starlight/types';

  const config: StarlightConfig;
  export default config;
}

declare module 'virtual:starlight/components/LanguageSelect' {
  export { default } from '@astrojs/starlight/components/LanguageSelect.astro';
}

declare module 'virtual:starlight/components/Search' {
  export { default } from '@astrojs/starlight/components/Search.astro';
}

declare module 'virtual:starlight/components/SiteTitle' {
  export { default } from '@astrojs/starlight/components/SiteTitle.astro';
}

declare module 'virtual:starlight/components/SocialIcons' {
  export { default } from '@astrojs/starlight/components/SocialIcons.astro';
}

declare module 'virtual:starlight/components/ThemeSelect' {
  export { default } from '@astrojs/starlight/components/ThemeSelect.astro';
}
