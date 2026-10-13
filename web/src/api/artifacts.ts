import type { Artifact } from "./types";

type ArtifactProvenance = Pick<Artifact, "path" | "tool"> & { purpose?: Artifact["purpose"] };

export function isOutputArtifact(artifact: ArtifactProvenance): boolean {
  if (artifact.purpose) return artifact.purpose === "output";
  // Old daemon logs have no purpose: root-level tool-call PNGs were observations.
  return !(!artifact.path.includes("/") && artifact.path.endsWith(".png") && (artifact.tool === "browser_screenshot" || artifact.path.startsWith("toolu_")));
}
