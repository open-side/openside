// Drives the official Bitwarden MV3 extension (popup opened as a tab) through CDP.
import { until, sleep } from "./lib.mjs";

export const BW_ID = "nngceckbapebfimnlniiiahkandclblb";
export const BW_ORIGIN = `chrome-extension://${BW_ID}`;

export class BitwardenUi {
  constructor(cdp, shotDir) {
    this.cdp = cdp;
    this.shotDir = shotDir;
  }

  /** Wraps an already open Bitwarden UI page (e.g. the desktop app's renderer). */
  attach(sessionId) {
    this.sid = sessionId;
    return this;
  }

  async open() {
    this.page = await this.cdp.page(`${BW_ORIGIN}/popup/index.html?uilocation=tab#/login`);
    this.sid = this.page.sessionId;
    await until("popup rendered", async () => (await this.text()).length > 0, 30000);
    return this;
  }

  ev(js) {
    return this.cdp.evaluate(this.sid, js);
  }

  text() {
    return this.ev("document.body.innerText");
  }

  hash() {
    return this.ev("location.hash");
  }

  setValue(id, value) {
    return this.ev(`(()=>{const e=document.getElementById(${JSON.stringify(id)}); if(!e) return false; e.focus();
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(value)});
      e.dispatchEvent(new Event('input',{bubbles:true})); e.dispatchEvent(new Event('change',{bubbles:true})); return true})()`);
  }

  click(text, { type } = {}) {
    return this.ev(`(()=>{const e=[...document.querySelectorAll('button,a')].find(e=>e.innerText.trim()===${JSON.stringify(text)}${type ? ` && e.type===${JSON.stringify(type)}` : ""}); if(!e) return false; e.click(); return true})()`);
  }

  async screenshot(name) {
    await this.cdp.screenshot(this.sid, `${this.shotDir}/${name}.png`);
  }

  /** Skips first-run prompts and logs in against a self-hosted server. Returns the final hash route. */
  async login({ server, email, password, doneRoute = "/tabs/" }) {
    await until("first-run or login screen", async () => {
      const h = await this.hash();
      if (h.includes("default-password-manager-prompt")) await this.click("Skip");
      if (h.includes("intro-carousel")) await this.click("Log in");
      return (await this.text()).includes("Email address");
    }, 30000, 500);
    await this.click("bitwarden.com");
    await sleep(500);
    await this.click("self-hosted");
    await until("server url field", () => this.setValue("self_hosted_env_settings_form_input_base_url", server));
    await this.click("Save", { type: "submit" });
    await until("env saved", async () => !(await this.text()).includes("Server URL"), 15000);
    await this.setValue("email", email);
    await this.click("Continue");
    await until("password field", () => this.setValue("masterPassword", password), 15000);
    await this.click("Log in", { type: "submit" });
    await until("vault route", async () => (await this.hash()).includes(doneRoute), 60000, 500);
    return this.hash();
  }
}
