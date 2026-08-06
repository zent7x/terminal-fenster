import { LINKS_BETWEEN, STACK } from '@/designs/content';

/**
 * The three-process architecture, as a real component.
 *
 * README.md draws this as an ASCII block. That reads fine in a terminal and
 * badly on a page, so it is rebuilt here as three layers with the wire between
 * them labelled — which is where the interesting claims actually live (0600
 * socket, no network listener, JSON control vs binary frames).
 */
export function ArchStack() {
  return (
    <div className="dk-arch">
      {STACK.map((layer, i) => (
        <div className="dk-arch-group" key={layer.id}>
          <div className="dk-arch-layer" data-role={layer.id}>
            <div className="dk-arch-head">
              <span className="dk-arch-name">{layer.name}</span>
              {layer.lang && <span className="dk-arch-lang">{layer.lang}</span>}
            </div>
            <p className="dk-arch-body">{layer.body}</p>
          </div>

          {i < STACK.length - 1 && (
            <div className="dk-arch-wire">
              <span className="dk-arch-rail" aria-hidden />
              <span className="dk-arch-wire-label">{LINKS_BETWEEN[i]}</span>
              <span className="dk-arch-rail" aria-hidden />
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
