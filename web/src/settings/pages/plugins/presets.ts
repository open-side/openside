import type { McpServerTransport } from "../../../api/types";
import type { MessageKey } from "../../../i18n";

export interface McpPreset {
  id: string;
  title: MessageKey;
  description: MessageKey;
  transport: McpServerTransport;
  /** stdio presets. */
  command?: string[];
  /** http presets. */
  url?: string;
}

/**
 * Well-known servers whose setup is public: each entry is exactly what the vendor documents, so
 * "+" only pre-fills the Connect form and the person still confirms it. Servers that only sign in
 * through a browser (OAuth, e.g. Granola) are left out: the daemon cannot complete that sign-in.
 */
export const MCP_PRESETS: readonly McpPreset[] = [
  {
    // Figma Desktop's local MCP server (Preferences › Enable desktop MCP server).
    id: "figma",
    title: "plugins.mcp.preset.figma.title",
    description: "plugins.mcp.preset.figma.description",
    transport: "http",
    url: "http://127.0.0.1:3845/mcp",
  },
  {
    // blender-mcp from PyPI, run by uv; it talks to the add-on running inside Blender.
    id: "blender",
    title: "plugins.mcp.preset.blender.title",
    description: "plugins.mcp.preset.blender.description",
    transport: "stdio",
    command: ["uvx", "blender-mcp"],
  },
];
