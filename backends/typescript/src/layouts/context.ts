/** Inputs shared by both bridge candidates. */

import type ts from "typescript";
import type { DataRootKind, PropertyScan, Registration, RegistrationScan } from "../ast.ts";
import type { RequestFragment } from "../protocol.ts";
import type { PragmaScan } from "../probes.ts";

export interface LayoutContext {
  /** Registration kind requested by the view owner. */
  rootKind: "page" | "component";
  /** Absolute path of the original script. */
  scriptPath: string;
  scriptText: string;
  /** Parsed with `setParentNodes` so AST helpers can walk parents. */
  sourceFile: ts.SourceFile;
  /** Registration scan of the original script. */
  scan: RegistrationScan;
  /** First registration matching the scenario kind. */
  registration?: Registration;
  /** How many registrations of the requested kind exist. */
  registrationCount: number;
  /** Classification of the `data` member (checker-based). */
  dataRootKind: DataRootKind;
  /** Declared component properties (may be incomplete on spread). */
  propertyScan: PropertyScan;
  /** Leading pragma scan of the original script. */
  pragma: PragmaScan;
  /** True when the policy asks for a shadow-only `@ts-check`. */
  wantPragma: boolean;
  /** Fragments from the request (the mount input, not the scenario table). */
  fragments: RequestFragment[];
  /** Effective JS check policy from the request. */
  jsCheckPolicy: "inherit" | "pragma" | "off";
  /** `checkJs` from the request's effective compiler options. */
  checkJs: boolean;
  /** True when the script is JavaScript. */
  isJavaScript: boolean;
}
