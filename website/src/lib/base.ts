/** Prefix a site path with Vite's configured base (needed for GitHub Pages). */
export function withBase(path: string): string {
  const base = import.meta.env.BASE_URL || '/';
  if (/^https?:\/\//i.test(path)) return path;
  const clean = path.replace(/^\//, '');
  return `${base}${clean}`;
}
