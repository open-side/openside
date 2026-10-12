import { describe, expect, it, vi } from "vitest";
import { createClient } from "./client";

const path = "/v1/sessions/s1/artifacts/report.html/preview?exp=1&sig=test";

describe("sandboxed previews over the selected transport", () => {
  it("fetches the signed preview through the injected transport and prepends CSP before untrusted HTML", async () => {
    const hostile = "<html><head><base href='https://untrusted.test/'></head><body><script>fetch('/v1/sessions')</script><h1>Report</h1></body></html>";
    const tunnel = vi.fn(async () => new Response(hostile, { headers: { "Content-Type": "text/html" } }));
    const client = createClient({ token: () => "secret", fetch: tunnel });
    const controller = new AbortController();
    const blob = await client.sandboxPreview(path, controller.signal);
    expect(tunnel).toHaveBeenCalledWith(path, { signal: controller.signal });
    expect(blob.type).toBe("text/html");
    const html = await blob.text();
    expect(html.indexOf("Content-Security-Policy")).toBeLessThan(html.indexOf(hostile));
    expect(html).toContain("default-src 'none'; script-src 'none'");
    expect(html).toContain("form-action 'none'; base-uri 'none'");
    expect(html).toContain(hostile);
  });

  it("escapes text previews instead of turning their contents into markup", async () => {
    const client = createClient({ token: () => "secret", fetch: vi.fn(async () => new Response("<script>&unsafe</script>", { headers: { "Content-Type": "text/plain" } })) });
    expect(await (await client.sandboxPreview(path)).text()).toContain("<pre>&lt;script&gt;&amp;unsafe&lt;/script&gt;</pre>");
  });

  it("preserves binary PDF bytes and MIME type for the sandboxed frame", async () => {
    const client = createClient({ token: () => "secret", fetch: vi.fn(async () => new Response("%PDF-test", { headers: { "Content-Type": "application/pdf" } })) });
    const blob = await client.sandboxPreview(path);
    expect(blob.type).toBe("application/pdf");
    expect(await blob.text()).toBe("%PDF-test");
  });
});
