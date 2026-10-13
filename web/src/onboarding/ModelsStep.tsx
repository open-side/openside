import { useCallback } from "react";
import type { ModelCatalog } from "../api/types";
import { ModelsPanel } from "../models/ModelsPanel";
import { providerLabel, providerReady } from "../models/providers";
import type { StepProps } from "./summary";

export function ModelsStep({ update }: StepProps) {
  const onCatalog = useCallback(
    (catalog: ModelCatalog) => {
      const ready = catalog.providers.filter(providerReady).map((provider) => providerLabel(provider.provider));
      update({ models: ready.length ? ready.join(", ") : null });
    },
    [update],
  );
  return <ModelsPanel compact onCatalog={onCatalog} />;
}
