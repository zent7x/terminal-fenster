import type { Metadata } from "next";
import { VivariumStage } from "./_components/VivariumStage";

export const metadata: Metadata = {
  title: "Vivarium — Wallpaper Profile Card",
  description:
    "A personal-site card floating on its own wallpaper: bio on the left, a glass-arcade scene on the right, blurred out behind the whole page. The scene is a frozen still until you point at it — the clip isn't even fetched before then. Recreated pixel-for-pixel from a reference.",
};

export default async function VivariumPage({
  searchParams,
}: {
  searchParams: Promise<{ still?: string }>;
}) {
  const sp = await searchParams;
  return <VivariumStage still={sp.still === "1"} />;
}
