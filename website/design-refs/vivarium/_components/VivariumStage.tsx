"use client";

import { Vivarium } from "./Vivarium";
import { OpenAIMark, PalantirMark, VercelMark, XMark } from "./marks";

/* The route's client entry. Kept separate from <Vivarium /> so the component
   itself can keep a real callback prop (`onSubscribe`) — a "use client" file
   imported straight from a server component may only take serialisable ones. */

export function VivariumStage({ still = false }: { still?: boolean }) {
  return (
    <Vivarium
      avatar="/vivarium/avatar.webp"
      avatarAlt="Hayden Bleasel"
      wallpaper={{
        still: "/vivarium/wallpaper.webp",
        clip: "/vivarium/wallpaper.mp4",
        alt: "A sunlit glass arcade over a lily pond, with whales drifting through the canopy",
      }}
      still={still}
      onSubscribe={(email) => {
        // Demo route — the field is real, the list isn't.
        if (process.env.NODE_ENV !== "production") {
          console.info("[vivarium] subscribe", email);
        }
      }}
    >
      <p>Hi, I&rsquo;m Hayden Bleasel.</p>
      <p>
        I work as a Member of Technical Staff at{" "}
        <OpenAIMark /> OpenAI. I&rsquo;m originally from Sydney, Australia; now
        living in San Francisco, California.
      </p>
      <p>
        I was previously acquihired at <VercelMark /> Vercel where I worked on
        the DX team. Before that, I was a CPO for a cybersecurity startup
        (acq&rsquo;d $170M), ran and sold my own agency, and interned at{" "}
        <PalantirMark /> Palantir.
      </p>
      <p>
        After hours I maintain OSS projects such as Ultracite, Ghost and Files
        SDK. I previously sold Kibo UI, Refraction and next-forge.
      </p>
      <p>
        If you&rsquo;d like to stay up to date with my adventures, subscribe to
        my mailing list below, follow me on <XMark /> or check out OS1.
      </p>
    </Vivarium>
  );
}
