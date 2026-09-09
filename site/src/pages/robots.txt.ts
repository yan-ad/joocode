import type { APIRoute } from 'astro';

export const prerender = true;

const robots = `User-agent: *
Allow: /

Sitemap: https://joocode.yan.ad/sitemap-index.xml
`;

export const GET: APIRoute = () =>
  new Response(robots, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
