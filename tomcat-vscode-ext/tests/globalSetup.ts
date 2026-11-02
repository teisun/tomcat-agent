import type { TestProject } from "vitest/node";
import { buildTomcatBinary } from "../scripts/buildTomcatBinary";

declare module "vitest" {
  export interface ProvidedContext {
    tomcatBuilt: boolean;
  }
}

export default async function setup(project: TestProject): Promise<void> {
  await buildTomcatBinary();
  // Only this successful run can skip compilation; file existence alone could be stale.
  project.provide("tomcatBuilt", true);
}
