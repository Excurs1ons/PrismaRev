import type { VizKey } from "../chapters";
import { mountFrameLoop } from "./frameLoop";
import { mountEcsFlow } from "./ecsFlow";
import { mountCoordSpace } from "./coordSpace";
import { mountDeployFlow } from "./deployFlow";
import { mountConcept } from "./concept";

export function mountViz(key: VizKey, host: HTMLElement): void {
  switch (key) {
    case "frameLoop":
      mountFrameLoop(host);
      break;
    case "ecsFlow":
      mountEcsFlow(host);
      break;
    case "coordSpace":
      mountCoordSpace(host);
      break;
    case "deployFlow":
      mountDeployFlow(host);
      break;
    case "memory":
    case "rendergraph":
    case "pipeline":
    case "coordchain":
      mountConcept(key, host);
      break;
  }
}
