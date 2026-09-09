import { createHighlighter, type Highlighter } from 'shiki';

let highlighter: Highlighter | undefined;
const cache = new Map<string, string>();

const LANGS = ['bash', 'powershell', 'toml', 'json', 'text'];

async function getHighlighter(): Promise<Highlighter> {
  if (!highlighter) {
    highlighter = await createHighlighter({
      themes: ['github-dark'],
      langs: LANGS,
    });
  }
  return highlighter;
}

export async function highlight(code: string, lang: string): Promise<string> {
  const key = `${lang}\n${code}`;
  const hit = cache.get(key);
  if (hit) return hit;

  const h = await getHighlighter();
  const safeLang = h.getLoadedLanguages().includes(lang) ? lang : 'text';
  const html = h.codeToHtml(code, { lang: safeLang, theme: 'github-dark' });
  cache.set(key, html);
  return html;
}
