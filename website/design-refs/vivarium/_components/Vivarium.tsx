"use client";

import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react";
import "../vivarium.css";

/* ==================================================================== */
/* Vivarium — a personal-site card floating on its own wallpaper.       */
/*                                                                      */
/* Self-contained: it renders the blurred page backdrop, the glass       */
/* frame, the white card, the bio column and the subscribe form. Drop    */
/* it on an empty route and you have the whole screen.                   */
/*                                                                      */
/* Everything is interaction-driven. The wallpaper is a still image      */
/* until you point at it (or press Enter on it); only then is the clip   */
/* even fetched, and it freezes again the moment you leave. Nothing on   */
/* this page moves by itself.                                            */
/* ==================================================================== */

export type VivariumWallpaper = {
  /** Still frame — the only thing loaded until the viewer interacts. */
  still: string;
  /** Optional motion clip. Fetched lazily, played only on interaction. */
  clip?: string;
  alt: string;
};

export type VivariumProps = {
  avatar: string;
  avatarAlt: string;
  /** Where the avatar links to. Omit for a non-interactive portrait. */
  avatarHref?: string;
  wallpaper: VivariumWallpaper;
  /** The bio copy — paragraphs, with inline marks where you want them. */
  children: ReactNode;
  placeholder?: string;
  /** Copy shown in the field after a successful submit. */
  sentLabel?: string;
  onSubscribe?: (email: string) => void;
  /** Render the resting state only — no clip, no hover affordances. */
  still?: boolean;
  className?: string;
};

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/;

function usePrefersReducedMotion() {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const sync = () => setReduced(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);
  return reduced;
}

export function Vivarium({
  avatar,
  avatarAlt,
  avatarHref,
  wallpaper,
  children,
  placeholder = "annie@acme.com",
  sentLabel = "You’re on the list.",
  onSubscribe,
  still = false,
  className,
}: VivariumProps) {
  const reduced = usePrefersReducedMotion();
  const hasClip = Boolean(wallpaper.clip) && !still;

  /* ---- wallpaper playback ------------------------------------------ */

  // `armed` gates the network: the clip's <video> isn't mounted (so the mp4
  // isn't fetched) until the viewer's pointer first enters the card.
  const [armed, setArmed] = useState(false);
  const [ready, setReady] = useState(false);
  const [hovering, setHovering] = useState(false);
  // Clicking pins the clip on, so it keeps running once the pointer leaves.
  const [pinned, setPinned] = useState(false);
  const [ctlVisible, setCtlVisible] = useState(false);

  const panelClip = useRef<HTMLVideoElement>(null);
  const backdropClip = useRef<HTMLVideoElement>(null);

  // Hover is a preview, and reduced-motion opts out of previews — but never
  // out of an explicit click, which is the viewer asking for it.
  const hoverWants = hovering && !reduced;
  const wants = hasClip && (pinned || hoverWants);
  const live = wants && ready;

  const arm = useCallback(() => {
    if (hasClip) setArmed(true);
  }, [hasClip]);

  useEffect(() => {
    const panel = panelClip.current;
    if (!panel) return;
    const back = backdropClip.current;

    if (live) {
      if (back) {
        // Start the wash from wherever the panel is so the two layers read
        // as one image rather than two takes of the same scene.
        if (Math.abs(back.currentTime - panel.currentTime) > 0.05) {
          back.currentTime = panel.currentTime;
        }
        void back.play().catch(() => {});
      }
      void panel.play().catch(() => {});
      return;
    }

    // Hold the frame until the crossfade back to the still has finished —
    // pausing immediately would show a hard stop under a fading layer.
    const hold = window.setTimeout(() => {
      panel.pause();
      back?.pause();
    }, 660);
    return () => window.clearTimeout(hold);
  }, [live]);

  const toggle = useCallback(() => {
    arm();
    setPinned((p) => !p);
  }, [arm]);

  /* ---- subscribe form ---------------------------------------------- */

  const [value, setValue] = useState("");
  const [focused, setFocused] = useState(false);
  const [sent, setSent] = useState(false);
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const timers = useRef<number[]>([]);
  const fieldId = useId();

  useEffect(
    () => () => {
      for (const t of timers.current) window.clearTimeout(t);
    },
    [],
  );

  const submit = useCallback(
    (event: React.SubmitEvent<HTMLFormElement>) => {
      event.preventDefault();
      if (sent) return;

      if (!EMAIL.test(value.trim())) {
        setInvalid(true);
        inputRef.current?.focus();
        timers.current.push(window.setTimeout(() => setInvalid(false), 460));
        return;
      }

      setInvalid(false);
      setSent(true);
      onSubscribe?.(value.trim());
      inputRef.current?.blur();
      timers.current.push(
        window.setTimeout(() => {
          setSent(false);
          setValue("");
        }, 2900),
      );
    },
    [onSubscribe, sent, value],
  );

  /* ---- render ------------------------------------------------------ */

  const showCtl = !still && (ctlVisible || pinned);

  return (
    <div
      className={className ? `viv-root ${className}` : "viv-root"}
      data-live={live ? "true" : "false"}
      onPointerEnter={arm}
    >
      {/* Plain <img>/<video> throughout: this is meant to be copied out of the
          repo whole, so it stays free of next/image. */}
      <div className="viv-backdrop" aria-hidden>
        <img className="viv-backdrop-media" src={wallpaper.still} alt="" />
        {armed && wallpaper.clip ? (
          <video
            ref={backdropClip}
            className="viv-backdrop-media viv-backdrop-media--clip"
            src={wallpaper.clip}
            muted
            loop
            playsInline
            preload="auto"
            tabIndex={-1}
          />
        ) : null}
      </div>

      <div className="viv-frame">
        <div className="viv-card">
          <div className="viv-col">
            {avatarHref ? (
              <a className="viv-avatar viv-avatar--link" href={avatarHref}>
                <img src={avatar} alt={avatarAlt} width={45} height={45} />
              </a>
            ) : (
              <span className="viv-avatar">
                <img src={avatar} alt={avatarAlt} width={45} height={45} />
              </span>
            )}

            <div className="viv-bio">{children}</div>

            <form
              className="viv-form"
              data-focused={focused ? "true" : "false"}
              data-sent={sent ? "true" : "false"}
              data-invalid={invalid ? "true" : "false"}
              onSubmit={submit}
              noValidate
            >
              <label className="viv-sr" htmlFor={fieldId}>
                Email address
              </label>
              <input
                id={fieldId}
                ref={inputRef}
                className="viv-input"
                type="email"
                name="email"
                autoComplete="email"
                spellCheck={false}
                placeholder={placeholder}
                value={value}
                onChange={(e) => {
                  setValue(e.target.value);
                  if (invalid) setInvalid(false);
                }}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
                disabled={sent}
              />
              <span className="viv-done" aria-live="polite">
                {sent ? sentLabel : ""}
              </span>
              <button
                className="viv-submit"
                type="submit"
                aria-label={sent ? "Subscribed" : "Subscribe"}
              >
                <svg
                  className="viv-submit-glyph viv-submit-glyph--arrow"
                  viewBox="0 0 16 16"
                  fill="none"
                  aria-hidden
                >
                  <path
                    d="M2.6 8h10.2M8.9 4.1 12.8 8l-3.9 3.9"
                    stroke="currentColor"
                    strokeWidth="1.7"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
                <svg
                  className="viv-submit-glyph viv-submit-glyph--check"
                  viewBox="0 0 16 16"
                  fill="none"
                  aria-hidden
                >
                  <path
                    d="M3.2 8.5 6.5 11.8l6.3-7.6"
                    stroke="currentColor"
                    strokeWidth="1.9"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              </button>
            </form>
          </div>

          <button
            type="button"
            className="viv-panel"
            data-live={live ? "true" : "false"}
            data-showctl={showCtl ? "true" : "false"}
            aria-pressed={pinned}
            aria-label={
              hasClip
                ? pinned
                  ? "Let the wallpaper settle back to a still"
                  : "Keep the wallpaper playing"
                : wallpaper.alt
            }
            disabled={!hasClip}
            onPointerEnter={() => {
              arm();
              setHovering(true);
              setCtlVisible(true);
            }}
            onPointerLeave={() => {
              setHovering(false);
              setCtlVisible(false);
            }}
            onFocus={() => {
              arm();
              setCtlVisible(true);
            }}
            onBlur={() => setCtlVisible(false)}
            onClick={toggle}
          >
            <img
              className="viv-panel-media viv-panel-media--still"
              src={wallpaper.still}
              alt={wallpaper.alt}
            />
            {armed && wallpaper.clip ? (
              <video
                ref={panelClip}
                className="viv-panel-media viv-panel-media--clip"
                src={wallpaper.clip}
                muted
                loop
                playsInline
                preload="auto"
                tabIndex={-1}
                onCanPlay={() => setReady(true)}
              />
            ) : null}
            <span className="viv-panel-scrim" aria-hidden />
            <span className="viv-panel-ctl" aria-hidden>
              {live ? (
                <>
                  <span className="viv-panel-dot" />
                  Live
                </>
              ) : (
                <>
                  <svg viewBox="0 0 12 12" fill="currentColor">
                    <path d="M2.6 1.5 10 6l-7.4 4.5Z" />
                  </svg>
                  Still
                </>
              )}
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}
