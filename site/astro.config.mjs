import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

export default defineConfig({
  site: 'https://joocode.yan.ad',
  output: 'static',
  trailingSlash: 'never',
  integrations: [sitemap()],
});
