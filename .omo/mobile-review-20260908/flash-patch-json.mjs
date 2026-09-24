import { createApplyPatchTool } from "/Users/indo/.bun/install/global/node_modules/@code-yeongyu/senpi/dist/core/extensions/builtin/gpt-apply-patch/tool.js";

export default function flashPatchJson(pi) {
  pi.on("session_start", () => {
    pi.registerTool(createApplyPatchTool("json"));
    pi.setActiveTools(["read", "apply_patch"]);
  });
}
