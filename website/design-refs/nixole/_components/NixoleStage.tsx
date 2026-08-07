"use client";

import { useEffect, useLayoutEffect } from "react";
import "../nixole.css";

/* ---- icons --------------------------------------------------------- */
function Chevron() {
  return (
    <svg className="chev" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M4 6l4 4 4-4"
        stroke="#9a9a9a"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

const LOGOS = [
  { cls: "oracle", label: "ORACLE" },
  { cls: "gofundme", label: "gofundme" },
  { cls: "nutanix", label: "NUTANIX" },
  { cls: "upside", label: "Upside" },
  { cls: "intel", label: "intel" },
];

export function NixoleStage({ still = false }: { still?: boolean }) {
  /* scale the 1600×1200 canvas to fit the viewport */
  useLayoutEffect(() => {
    const stage = document.querySelector(".nx-stage") as HTMLElement | null;
    if (!stage) return;
    const fit = () => {
      const s = Math.min(window.innerWidth / 1600, window.innerHeight / 1200);
      stage.style.setProperty("--nx-scale", String(s > 0 ? s : 1));
    };
    fit();
    window.addEventListener("resize", fit);
    return () => window.removeEventListener("resize", fit);
  }, []);

  /* inert the global studio chrome while the stage is up */
  useEffect(() => {
    const header = document.querySelector("body > header");
    header?.setAttribute("inert", "");
    return () => header?.removeAttribute("inert");
  }, []);

  return (
    <div className={`nx-stage ${still ? "nx-still" : "nx-anim"}`}>
      <div className="nx-canvas">
        <div className="nx-page">
          {/* header */}
          <header className="nx-header">
            <div className="nx-logo nx-rise" style={{ "--d": "0.10s" } as React.CSSProperties}>
              <img className="nx-logo-icon" src="/nixole/icon.png" alt="" />
              <span className="nx-logo-word">Nixole</span>
            </div>

            <nav className="nx-nav" style={{ "--d": "0.18s" } as React.CSSProperties}>
              <a className="active" href="#">
                Home
              </a>
              <a href="#">How it works</a>
              <a href="#">
                Company
                <Chevron />
              </a>
              <a href="#">Case Studies</a>
            </nav>

            <button
              className="nx-book nx-rise"
              type="button"
              style={{ "--d": "0.24s" } as React.CSSProperties}
            >
              Book a demo
            </button>
          </header>

          {/* badge */}
          <div className="nx-badge" style={{ "--d": "0.38s" } as React.CSSProperties}>
            <span className="chip">AI</span>
            <span className="blabel">Autonomous calls, enrollment &amp; payments</span>
          </div>

          {/* headline */}
          <h1 className="nx-h1">
            <span
              className="line nx-rise"
              style={{ "--d": "0.52s" } as React.CSSProperties}
            >
              AI that converts patients
            </span>
            <span
              className="line l2 nx-rise"
              style={{ "--d": "0.66s" } as React.CSSProperties}
            >
              for your
              <img className="nx-capsule" src="/nixole/capsule.png" alt="" />
              <span className="muted">medical practice</span>
            </span>
          </h1>

          {/* subtext */}
          <p className="nx-sub nx-rise" style={{ "--d": "0.82s" } as React.CSSProperties}>
            Built for how patients actually decide. They Google. They hesitate. They
            miss calls.
            <br />
            Our AI handles every touchpoint - calls, WhatsApp - until they book an
            appointment.
          </p>

          {/* cta */}
          <div className="nx-cta" style={{ "--d": "0.96s" } as React.CSSProperties}>
            <button className="ghost" type="button">
              Book a demo
            </button>
            <button className="solid" type="button">
              Let&apos;s Talk
            </button>
          </div>

          {/* hero card */}
          <div className="nx-hero nx-rise" style={{ "--d": "1.12s" } as React.CSSProperties}>
            <img className="nx-hero-img" src="/nixole/hero.png" alt="" />
            <div className="nx-strip" />
            <div className="nx-trusted">
              Trusted by industry leaders in X who don&apos;t just follow trends, but
              define how the industry moves forward.
            </div>
            <div className="nx-marquee">
              <div className="nx-track">
                {[...LOGOS, ...LOGOS].map((l, i) => (
                  <span key={i} className={`nx-logo-mark ${l.cls}`}>
                    {l.label}
                  </span>
                ))}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
