import { describe, expect, it } from "vitest";
import { htmlDocument, htmlPicture, svgPicture } from "./visualImage";

const decode = (url: string) => decodeURIComponent(url.slice(url.indexOf(",") + 1));

describe("visual images", () => {
  it("gives an SVG the page's ink and font, drops its scripts, and keeps its own width", () => {
    const picture = svgPicture('<svg xmlns="http://www.w3.org/2000/svg" width="320" height="120"><script>alert(1)</script><text style="font-weight:600">hi</text></svg>', "rgb(1, 2, 3)")!;
    const svg = decode(picture.url);
    expect(picture.url).toMatch(/^data:image\/svg\+xml/);
    expect(svg).not.toContain("<script");
    expect(svg).toContain("color:rgb(1, 2, 3)");
    expect(svg).toContain("system-ui");
    expect(svg).toContain('<text style="font-weight:600">hi</text>');
    expect(picture.width).toBe(320);
    expect(svgPicture('<svg xmlns="http://www.w3.org/2000/svg" width="100%" viewBox="0 0 4 2"/>', "red")!.width).toBeUndefined();
    expect(svgPicture('<svg viewBox="0 0 400 180"><rect width="4" height="2"/></svg>', "red")!.width).toBe(400);
  });

  it("refuses what is not an SVG document", () => {
    expect(svgPicture("<svg><rect></svg", "red")).toBeNull();
    expect(svgPicture("<div>hi</div>", "red")).toBeNull();
  });

  it("turns HTML into inert XHTML: no script, frames or loads; its own styles after the defaults", () => {
    const xhtml = htmlDocument(
      '<!doctype html><html><head><meta http-equiv="refresh" content="0;url=x"><link rel="stylesheet" href="x.css"><style>.bar > i{fill:red}</style></head><body style="margin:4px"><div class="bar">A &amp; B</div><script>parent.x=1</script><iframe src="x"></iframe></body></html>',
      "rgb(9, 9, 9)",
    );
    expect(xhtml).toMatch(/^<html xmlns="http:\/\/www\.w3\.org\/1999\/xhtml"><head><style>/);
    expect(xhtml).not.toMatch(/<script|<iframe|<meta|<link|osv-style/);
    expect(xhtml.indexOf("background:transparent")).toBeLessThan(xhtml.indexOf(".bar &gt; i{fill:red}"));
    expect(xhtml).toContain("color:rgb(9, 9, 9)");
    expect(xhtml).toMatch(/<body [^>]*style="margin:4px"/);
    expect(xhtml).toContain("A &amp; B");
    const svg = decode(htmlPicture(xhtml, 600, 240));
    expect(svg).toContain('<foreignObject x="0" y="0" width="600" height="240">');
    const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
    expect(parsed.getElementsByTagName("parsererror")).toHaveLength(0);
    expect(parsed.getElementsByTagName("style")[1]?.textContent).toBe(".bar > i{fill:red}");
  });
});
