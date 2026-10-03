// Real production App/Tiptap + simulated host. Does not replace real Serve/VS Code tests.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBrowserPath, resolveLaunchOptions } from "../../tomcat/assets/skills/verify/scripts/browser-path.mjs";
const root = fileURLToPath(new URL("../", import.meta.url));
const verify = new URL("../../tomcat/assets/skills/verify/scripts/shot.mjs", import.meta.url);
const index = process.argv.indexOf("--out");
assert.ok(index >= 0 && process.argv[index+1], "Pass --out <project_resource_dir>/shots/commands-composer");
const out = path.resolve(process.argv[index+1]);
await mkdir(out,{recursive:true});
process.env.PLAYWRIGHT_BROWSERS_PATH = resolveBrowserPath(verify.href);
const {chromium} = createRequire(verify)("playwright");
const commands = [
  {name:"reload",usage:"/reload",summary:"重新核对磁盘上的 Skill 与插件"},
  {name:"install",usage:"/install <来源> <current-project|agent|global>",summary:"安装 package / 插件 / Skill"},
  {name:"uninstall",usage:"/uninstall <包名> <current-project|agent|global>",summary:"卸载 package"},
];
const catalog = [
  ...Array.from({length:25},(_,n)=>({id:`skill:skill-${n}`,kind:"skill",name:`skill-${String(n).padStart(2,"0")}`,description:"Create a focused workflow for the current task",source:"managed",path:`/fixture/skills/${n}/SKILL.md`})),
  {id:"command:.cursor/commands/review.md",kind:"command",name:"review",description:"按项目规范检查当前修改",source:".cursor",path:".cursor/commands/review.md"},
  {id:"command:.agents/commands/review.md",kind:"command",name:"review",description:"",source:".agents",path:".agents/commands/review.md"},
];
function fixture() { return {
  activeSessionId:"s1",availableModels:["gpt-5.4"],ready:true,mediaRoots:[],slashCommands:commands,
  sessions:[{sessionId:"s1",title:"Commands / Composer 验收",isCurrent:true,ownedByThisFrontend:true,busy:false,updatedAt:1}],
  sessionViews:{s1:{sessionId:"s1",ownedByThisFrontend:true,busy:false,model:"gpt-5.4",agentMode:"chat",thinkingLevel:"high",contextRatio:0.04,activePlan:null,planTodos:[],sessionTodos:[],pendingAttachments:[],instructionCatalog:catalog,timeline:[{type:"message",id:"assistant",kind:"assistant",text:"历史消息始终可以查看。"}]}},
}; }
function theme(light,fontSize=13) {
  return `:root { --vscode-font-family:-apple-system,BlinkMacSystemFont,sans-serif; --vscode-font-size:${fontSize}px; --vscode-foreground:${light?"#333333":"#cccccc"}; --vscode-descriptionForeground:${light?"#616161":"#999999"}; --vscode-sideBar-background:${light?"#f3f3f3":"#181818"}; --vscode-editor-background:${light?"#ffffff":"#181818"}; --vscode-editorWidget-background:${light?"#ffffff":"#202020"}; --vscode-editorWidget-border:#88888855; --vscode-panel-border:#88888855; --vscode-input-background:${light?"#ffffff":"#202020"}; --vscode-input-foreground:${light?"#333333":"#cccccc"}; --vscode-input-placeholderForeground:#888888; --vscode-dropdown-background:${light?"#ffffff":"#202020"}; --vscode-dropdown-foreground:${light?"#333333":"#cccccc"}; --vscode-dropdown-border:#88888855; --vscode-list-activeSelectionBackground:${light?"#e4e4e4":"#333333"}; --vscode-button-background:#267fa4; --vscode-button-foreground:#ffffff; --vscode-focusBorder:#267fa4; --vscode-scrollbarSlider-background:#88888866; } body { padding:0 20px; font-size:${fontSize}px; }`;
}
const scenarios=[
  {name:"narrow-dark",width:390,height:844}, {name:"desktop-light",width:1440,height:900,light:true},
  {name:"narrowest",width:260,height:480}, {name:"short",width:320,height:400},
  {name:"bottom-panel",width:390,height:280}, {name:"large-font",width:320,height:600,fontSize:18},
  ...[0.8,1.5,2].map(zoom=>({name:`zoom-${zoom}`,width:Math.round(780/zoom),height:Math.round(900/zoom),zoom})),
];
const scenarioIndex = process.argv.indexOf("--scenario");
const selectedScenario = scenarioIndex >= 0 ? process.argv[scenarioIndex + 1] : null;
assert.ok(!selectedScenario || scenarios.some((scenario) => scenario.name === selectedScenario), "Unknown --scenario");
const vite=spawn(process.execPath,[path.join(root,"gui/node_modules/vite/bin/vite.js"),"--host","127.0.0.1","--port","0"],{cwd:path.join(root,"gui"),stdio:["ignore","pipe","pipe"]});
let browser;
try {
  const url=await new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>reject(new Error("Vite readiness timed out")),30000); let text="";
    vite.stdout.on("data",chunk=>{text+=chunk; const match=text.match(/http:\/\/127\.0\.0\.1:\d+\//); if(match){clearTimeout(timer);resolve(match[0]);}});
    vite.stderr.on("data",chunk=>process.stderr.write(chunk));
    vite.once("error",reject); vite.once("exit",code=>reject(new Error(`Vite exited ${code}`)));
  });
  console.log(`Vite ready: ${url}`);
  browser=await chromium.launch({headless:true,ignoreDefaultArgs:["--hide-scrollbars"],...await resolveLaunchOptions(verify.href)});
  for (const scenario of scenarios.filter((scenario) => !selectedScenario || scenario.name === selectedScenario)) {
    const page=await browser.newPage({viewport:{width:scenario.width,height:scenario.height},deviceScaleFactor:scenario.zoom??1});
    page.setDefaultTimeout(15000); const events=[];
    page.on("console",message=>events.push({level:message.type(),text:message.text()}));
    page.on("pageerror",error=>events.push({level:"error",text:error.message}));
    await page.addInitScript(state=>{
      window.__fixture=state; window.__intents=[]; window.__seq=0;
      window.__emitState=()=>window.postMessage({channel:"state",content:window.__fixture,messageId:`fixture-${++window.__seq}`},"*");
      let saved;
      window.acquireVsCodeApi=()=>({getState:()=>saved,setState:v=>{saved=v;},postMessage:message=>{
        window.__intents.push(message);
        if(message.type === "ready") window.__emitState();
        if(message.type === "searchContext") window.postMessage({channel:"event",content:{type:"contextSearchResult",requestId:message.data.requestId,query:message.data.query,sessionId:message.data.sessionId,truncated:false,matches:[{reference:{type:"reference",kind:"file",label:"app.ts",path:"src/app.ts"},description:"src"}]},messageId:`search-${++window.__seq}`},"*");
        if(message.type === "prompt") {
          const d=message.data;
          window.__fixture.sessionViews.s1.timeline.push({type:"message",kind:"user",id:d.userMessageId,text:d.text,segments:d.segments});
          window.__fixture.sessionViews.s1.composerDraft={text:"",segments:[]}; window.__emitState();
        }
      }});
    },fixture());
    await page.goto(url,{waitUntil:"domcontentloaded"});
    await page.addStyleTag({content:theme(scenario.light,scenario.fontSize)});
    await page.evaluate(light=>{document.body.classList.add("tc-chat-webview",light?"vscode-light":"vscode-dark");},!!scenario.light);
    const input=page.getByTestId("composer-input"); await input.waitFor();
    await page.evaluate(()=>document.fonts.ready);
    const replace=async value=>{
      await input.click();
      await page.keyboard.press(process.platform === "darwin" ? "Meta+A" : "Control+A");
      await page.keyboard.press("Backspace");
      // Real paste preserves hard breaks and avoids depending on keyboard.type newline behavior.
      await input.evaluate((el,text)=>{const data=new DataTransfer();data.setData("text/plain",text);el.dispatchEvent(new ClipboardEvent("paste",{bubbles:true,clipboardData:data}));},value);
    };
    const geometry=()=>page.evaluate(()=>{
      const rect=selector=>{const el=document.querySelector(selector);if(!el)return null;const r=el.getBoundingClientRect(),cs=getComputedStyle(el);return {left:r.left,right:innerWidth-r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height,lineHeight:parseFloat(cs.lineHeight),padding:cs.padding,scrollHeight:el.scrollHeight,clientHeight:el.clientHeight,scrollTop:el.scrollTop};};
      return {viewport:[innerWidth,innerHeight],bodyPadding:getComputedStyle(document.body).padding,controls:parseFloat(getComputedStyle(document.body).getPropertyValue('--tc-controls-inset'))||10,content:parseFloat(getComputedStyle(document.body).getPropertyValue('--tc-content-inset'))||15,surface:rect('[data-testid="composer-surface"]'),editor:rect('.tc-composer__editor'),toolbar:rect('[data-testid="composer-bar"]'),firstButton:rect('[data-testid="attachment-add"]'),topbar:rect('.tc-topbar'),stream:rect('.tc-stream'),menu:rect('[data-testid="slash-command-menu"]'),mention:rect('[data-testid="context-search-dropdown"]')};
    });
    const capture=async suffix=>{
      const name=`${scenario.name}-${suffix}`,g=await geometry();
      await page.screenshot({path:path.join(out,`${name}.png`)});
      await writeFile(path.join(out,`${name}.aria.txt`),await page.locator('body').ariaSnapshot());
      await writeFile(path.join(out,`${name}.console.json`),JSON.stringify(events,null,2));
      await writeFile(path.join(out,`${name}.geometry.json`),JSON.stringify(g,null,2));
      assert.equal(events.filter(e=>e.level === "error").length,0,JSON.stringify(events));
      assert.equal(g.bodyPadding,"0px");
      assert.ok(Math.abs(g.topbar.left-g.controls)<=1 && Math.abs(g.topbar.right-g.controls)<=1,"Session bar/control alignment");
      assert.ok(Math.abs(g.stream.left-g.content)<=1,"Transcript/content alignment");
      assert.ok(Math.abs(g.surface.left-g.controls)<=1 && Math.abs(g.surface.right-g.controls)<=1,JSON.stringify(g));
      assert.ok(Math.abs(g.firstButton.left-g.content)<=1 && Math.abs(g.editor.left-g.content)<=1,"Text/button/content alignment");
      assert.ok(g.editor.height<=g.viewport[1]*0.3+1,"30vh editor limit");
      assert.ok(g.toolbar.top>=0 && g.toolbar.bottom<=g.viewport[1]+1,"Toolbar visible");
      assert.equal(g.stream.right,0,"Scrollbar edge");
      if(g.menu){assert.ok(Math.abs(g.menu.left-g.editor.left)<=1,"Menu text alignment");assert.ok(g.menu.right>=g.content-1,"Menu right border visible");assert.ok(g.menu.top>=0,"Menu top visible");assert.ok(g.menu.width<=260+1);}
      if(g.mention){assert.ok(Math.abs(g.mention.left-g.editor.left)<=1,"Mention text alignment");assert.ok(g.mention.right>=g.content-1,"Mention right border");}
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),"No horizontal page overflow");
      console.log(`Captured ${name}`);return g;
    };
    const empty=await capture("empty");
    assert.ok(Math.abs(empty.editor.height-empty.editor.lineHeight-8)<=1,"One-line minimum");
    await replace("/"); await page.getByTestId("slash-command-menu").waitFor();
    if(scenario.height>=600){for(const title of ["Skills","Commands","Terminal"]) {const r=await page.getByRole('group',{name:title,exact:true}).boundingBox();assert.ok(r.y>=0 && r.y<scenario.height,"Every group starts on the first screen");}}
    await capture("menu");
    const singleMenu = await geometry();
    assert.ok(singleMenu.menu, "Single-line slash menu exists");
    if (scenario.name === "bottom-panel") {
      assert.ok(singleMenu.menu.height >= Math.min(singleMenu.menu.scrollHeight + 2, singleMenu.surface.top - 16) - 1, "Single-line menu must use available space");
    }
    await page.getByTestId("slash-command-more").click();
    assert.equal(await page.getByTestId("slash-command-option").count(),30);
    await page.keyboard.press("Escape");
    await replace("/review"); await page.getByTestId("slash-command-menu").waitFor();
    await page.keyboard.press("Enter"); await input.getByTestId("invocation-chip").waitFor();
    assert.equal(await input.getByTestId("invocation-chip").locator('button').count(),0);
    await capture("chip");
    const color=await input.getByTestId("invocation-chip").evaluate(el=>getComputedStyle(el).color);
    assert.equal(color,scenario.light ? "rgb(122, 79, 0)" : "rgb(241, 181, 103)");
    await page.getByTestId("send-button").click();
    await page.locator('[data-kind="user"]').getByTestId("invocation-chip").waitFor();
    assert.equal(await page.locator('body').getByText('<command',{exact:false}).count(),0);
    await capture("history-chip");
    await replace("help /"); await page.getByTestId("slash-command-menu").waitFor();
    assert.equal(await page.getByRole('group',{name:'Terminal',exact:true}).count(),0);
    await page.keyboard.press("Escape");
    await replace("/reload");await page.getByTestId("slash-command-menu").waitFor();await page.keyboard.press("Enter");
    assert.equal((await input.innerText()).trim(),"/reload");await page.getByTestId("send-button").click();
    assert.ok(await page.evaluate(()=>window.__intents.some(i=>i.type === "runSlashCommand")));
    await replace("@app"); await page.getByTestId("context-search-dropdown").waitFor(); await capture("mention"); await page.keyboard.press("Escape");
    await replace("Long composer line\n".repeat(100));
    const long=await capture("long");assert.ok(long.editor.scrollHeight>long.editor.clientHeight);
    await replace("Long composer line\n".repeat(100) + "/");
    await page.getByTestId("slash-command-menu").waitFor();
    const longMenu = await geometry();
    // Capture before assertions so a red run retains the clipped geometry and PNG.
    await capture("long-menu");
    assert.ok(longMenu.menu, "Long-input slash menu exists");
    assert.ok(longMenu.editor.scrollHeight > longMenu.editor.clientHeight && Math.abs(longMenu.editor.height - scenario.height * 0.3) <= 1, "Editor must be at its scroll limit");
    assert.ok(longMenu.menu.top >= 0 && longMenu.menu.bottom <= longMenu.surface.top - 7, "Long-input menu fits above composer");
    await page.keyboard.press("Escape");
    await replace("Long composer line\n".repeat(100));
    await page.evaluate(()=>{for(let n=0;n<30;n++)window.postMessage({channel:"event",content:{type:"insertReference",sessionId:"s1",reference:{type:"reference",kind:"file",path:"src/app.ts",label:"app.ts",occurrenceId:`repeat-${n}`}},messageId:`repeat-${n}`},"*");});
    await page.getByTestId('composer-reference-chip').nth(29).waitFor();
    await capture("long-references");
    const sizes=[long.editor.height,long.surface.height];
    await page.evaluate(()=>{window.__fixture.sessionViews.s1.pendingAttachments=Array.from({length:11},(_,n)=>({id:`att-${n}`,blobSha:"fixture",kind:"file",mimeType:"application/pdf",filename:`file-${n}.pdf`,label:`file-${n}.pdf`,path:"/fixture/file.pdf"}));window.__emitState();});
    await page.getByRole('list',{name:'Pending attachments',exact:true}).waitFor();
    const attached=await capture("long-attachments");assert.deepEqual([attached.editor.height,attached.surface.height],sizes);
    await replace("");await capture("collapsed");
    await page.evaluate(()=>{window.__fixture.sessionViews.s1.pendingAttachments=[];window.__emitState();document.body.style.setProperty('--tc-controls-inset','4px');document.body.style.setProperty('--tc-content-inset','24px');});
    await replace("/");await page.getByTestId("slash-command-menu").waitFor();await capture("custom-insets");
    await page.close();
  }
} finally {
  await browser?.close();
  if(vite.exitCode === null){const exited=new Promise(resolve=>vite.once('exit',resolve));vite.kill('SIGTERM');await exited;}
}
