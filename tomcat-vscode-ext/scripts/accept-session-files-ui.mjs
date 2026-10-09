// Real App/CSS with a simulated host. Native diff and Rust bytes are tested separately.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveBrowserPath, resolveLaunchOptions } from "../../tomcat/assets/skills/verify/scripts/browser-path.mjs";
const root=fileURLToPath(new URL("../",import.meta.url));
const verifyUrl=new URL("../../tomcat/assets/skills/verify/scripts/shot.mjs",import.meta.url);
const outIndex=process.argv.indexOf("--out");assert.ok(outIndex>=0,"Pass --out <artifact-directory>");
const out=path.resolve(process.argv[outIndex+1]);await mkdir(out,{recursive:true});
process.env.PLAYWRIGHT_BROWSERS_PATH=resolveBrowserPath(verifyUrl.href);
const {chromium}=createRequire(verifyUrl)("playwright");
function fixture(scenario) {
  const files=Array.from({length:scenario.long?120:4},(_,i)=>({path:`/workspace/src/file-${i}.ts`,displayPath:`src/file-${i}.ts`,status:"modified",added:i+2,removed:1,restorable:i!==2,blockedReason:i===2?"backup_missing":undefined}));
  const session={sessionId:"s1",ownedByThisFrontend:true,busy:!!scenario.busy,model:"gpt-5.4",agentMode:"chat",activePlan:null,planTodos:[],sessionTodos:scenario.busy?Array.from({length:20},(_,i)=>({id:`t${i}`,content:`Task ${i+1}`,status:i===0?"in_progress":"pending"})):[],pendingAttachments:[],composerDraft:{text:"Keep this draft\nSecond line",segments:[{type:"text",text:"Keep this draft\nSecond line"}]},sessionFiles:scenario.todos?undefined:scenario.empty?{sourceTurnId:null,files:[]}:scenario.error?{sourceTurnId:null,files:[],error:"Unavailable"}:{sourceTurnId:"u1",files},timeline:Array.from({length:40},(_,i)=>({id:`m${i}`,type:"message",kind:i%2?"assistant":"user",text:`History ${i}: ${"Conversation context. ".repeat(8)}`}))};
  if(scenario.question){session.pendingAttachments=[{id:"file",kind:"file",blobSha:"fixture",mimeType:"text/plain",filename:"README.md",label:"README.md",path:"/workspace/README.md"}];session.timeline.push({id:"approval",type:"approval",live:true,resolved:false,sessionId:"s1",request:{requestId:"r1",responseEvent:"answer",questions:[{id:"q",prompt:"Which approach should be used?",options:[{id:"a",label:"Keep the simpler option",recommended:true},{id:"b",label:"Choose the alternative"}]}]}});}
  return {activeSessionId:"s1",sessionFilesSupported:true,availableModels:["gpt-5.4"],ready:true,modelAdminSupported:false,mediaRoots:[],sessions:[{sessionId:"s1",title:"Files acceptance",ownedByThisFrontend:true,isCurrent:true,busy:session.busy},{sessionId:"s2",title:"Other session",ownedByThisFrontend:true,isCurrent:false,busy:false}],sessionViews:{s1:session,s2:{...session,sessionId:"s2",busy:false,sessionTodos:[],composerDraft:{text:"Other draft",segments:[{type:"text",text:"Other draft"}]},timeline:[],pendingAttachments:[]}}};
}
function theme(light){return `:root{--vscode-font-family:-apple-system,BlinkMacSystemFont,sans-serif;--vscode-font-size:13px;--vscode-foreground:${light?"#333":"#ccc"};--vscode-descriptionForeground:${light?"#616161":"#999"};--vscode-disabledForeground:#777;--vscode-sideBar-background:${light?"#f3f3f3":"#181818"};--vscode-editor-background:${light?"#fff":"#181818"};--vscode-editorWidget-background:${light?"#fff":"#202020"};--vscode-editorWidget-border:${light?"#c8c8c8":"#333"};--vscode-panel-border:${light?"#c8c8c8":"#333"};--vscode-input-background:${light?"#fff":"#202020"};--vscode-input-foreground:${light?"#333":"#ccc"};--vscode-focusBorder:#267fa4;--vscode-charts-green:${light?"#28752b":"#73c991"};--vscode-errorForeground:#f14c4c;--vscode-list-hoverBackground:${light?"#e4e4e4":"#2a2d2e"};--vscode-scrollbarSlider-background:#88888866;--vscode-button-background:#267fa4;--vscode-button-foreground:#fff}body{font-size:13px}`;}
const vite=spawn(process.execPath,[path.join(root,"gui/node_modules/vite/bin/vite.js"),"--host","127.0.0.1","--port","0"],{cwd:path.join(root,"gui"),stdio:["ignore","pipe","pipe"]});
let browser;
const results=[];
try {
  const url=await new Promise((resolve,reject)=>{let output="";const timer=setTimeout(()=>reject(new Error("Vite readiness timeout")),30_000);vite.stdout.on("data",chunk=>{output+=chunk.toString();const match=output.match(/http:\/\/127\.0\.0\.1:\d+\//);if(match){clearTimeout(timer);resolve(match[0]);}});vite.stderr.on("data",chunk=>process.stderr.write(chunk));vite.once("error",reject);});
  console.log(`Vite ready: ${url}`);browser=await chromium.launch({headless:true,ignoreDefaultArgs:["--hide-scrollbars"],...await resolveLaunchOptions(verifyUrl.href)});
  const scenarios=[{name:"wide-files",width:1440,height:900},{name:"narrow-files",width:390,height:844},{name:"narrow-light",width:390,height:844,light:true},{name:"busy-both",width:390,height:844,busy:true},{name:"short-question",width:390,height:320,busy:true,question:true,long:true},{name:"long-files",width:390,height:844,long:true},{name:"todo-only",width:390,height:844,busy:true,todos:true},{name:"tight-menu",width:289,height:844,empty:true,menu:true},{name:"empty",width:390,height:844,empty:true},{name:"error",width:390,height:844,error:true}];
  for(const scenario of scenarios){
    const page=await browser.newPage({viewport:{width:scenario.width,height:scenario.height}});page.setDefaultTimeout(15_000);
    const consoleEvents=[];page.on("console",msg=>consoleEvents.push({level:msg.type(),text:msg.text()}));page.on("pageerror",e=>consoleEvents.push({level:"error",text:e.message}));
    await page.addInitScript(state=>{window.__state=state;window.__intents=[];let saved;let counter=0;window.__emit=()=>window.postMessage({channel:"state",content:window.__state,messageId:`fixture-${++counter}`},"*");window.acquireVsCodeApi=()=>({getState:()=>saved,setState:v=>{saved=v;},postMessage:intent=>{window.__intents.push(intent);if(intent.type==="ready")window.__emit();if(intent.type==="switchSession"){window.__state.activeSessionId=intent.data.sessionId;window.__emit();}if(intent.type==="syncComposerDraft"){window.__state.sessionViews[intent.data.sessionId].composerDraft={text:intent.data.text,segments:intent.data.segments};}if(intent.type==="keepSessionFiles")setTimeout(()=>{const session=window.__state.sessionViews[intent.data.sessionId];session.sessionFiles={sourceTurnId:"keep-1",files:[]};window.__emit();window.postMessage({channel:"event",messageId:`result-${++counter}`,content:{type:"keepSessionFilesResult",...intent.data,success:true}},"*");},30);if(intent.type==="restoreSessionFiles")setTimeout(()=>{const session=window.__state.sessionViews[intent.data.sessionId];session.sessionFiles.files=session.sessionFiles.files.filter(f=>!intent.data.paths.includes(f.path));window.__emit();window.postMessage({channel:"event",messageId:`result-${++counter}`,content:{type:"restoreSessionFilesResult",...intent.data,success:true}},"*");},30);}});},fixture(scenario));
    await page.goto(url);await page.addStyleTag({content:theme(scenario.light)});await page.evaluate(light=>document.body.classList.add("tc-chat-webview",light?"vscode-light":"vscode-dark"),!!scenario.light);
    await page.getByTestId("composer-input").waitFor();await page.evaluate(()=>document.fonts.ready);
    const capture=async suffix=>{const name=`${scenario.name}-${suffix}`;await page.screenshot({path:path.join(out,`${name}.png`)});await writeFile(path.join(out,`${name}.aria.txt`),await page.locator("body").ariaSnapshot());await writeFile(path.join(out,`${name}.console.json`),JSON.stringify(consoleEvents,null,2));};
    try{
      if(scenario.empty){assert.equal(await page.getByTestId("session-dock").count(),0);}
      else if(scenario.error){assert.equal(await page.getByText("Couldn't load file changes.",{exact:false}).count(),1);}
      else if(scenario.todos){await page.getByRole("button",{name:"Expand todos"}).click();assert.equal(await page.getByTestId("todo-widget-item").count(),20);}
      else {
        await page.getByTestId("files-toggle").click();
        await page.getByTestId("session-files-list").waitFor();
        const row=page.getByTestId("session-file-row").first();const undo=row.getByTestId("undo-file");
        await page.mouse.move(0,0);await undo.evaluate(button=>button.blur());
        assert.equal(await undo.evaluate(button=>getComputedStyle(button).opacity),"0");
        await row.hover();assert.equal(await undo.evaluate(button=>getComputedStyle(button).opacity),"1");
        assert.notEqual(await row.evaluate(node=>getComputedStyle(node).backgroundColor),"rgba(0, 0, 0, 0)");
        assert.equal(await page.getByTestId("undo-all-files").isDisabled(),!!scenario.busy);
        assert.equal(await page.getByTestId("keep-all-files").isDisabled(),!!scenario.busy);
        const title=await page.getByTestId("files-title").boundingBox(),keep=await page.getByTestId("keep-all-files").boundingBox(),undoAll=await page.getByTestId("undo-all-files").boundingBox();
        assert.ok(title.x+title.width<=keep.x+1 && keep.x+keep.width<=undoAll.x+1 && undoAll.x+undoAll.width<=scenario.width,"Files title and both actions must not overlap or overflow");
        if(!scenario.busy){
          await undo.click();await page.getByTestId("undo-files-dialog").waitFor();
          const actions=await page.evaluate(()=>window.__intents.filter(i=>i.type==="openSessionFileDiff" || i.type==="restoreSessionFiles"));assert.equal(actions.length,0,"Undo opens confirmation, not diff or mutation");
          await page.keyboard.press("Escape");await row.getByTestId("session-file-diff").focus();assert.equal(await undo.evaluate(button=>getComputedStyle(button).opacity),"1");
        }
        if(scenario.long){await page.getByTestId("session-files-list").evaluate(list=>{list.scrollTop=list.scrollHeight;});await page.getByTestId("session-file-row").last().scrollIntoViewIfNeeded();await page.getByTestId("session-files-list").evaluate(list=>{list.scrollTop=0;});}
      }
      const geometry=await page.evaluate(()=>{const rect=selector=>{const n=document.querySelector(selector);if(!n)return null;const r=n.getBoundingClientRect();return {top:r.top,bottom:r.bottom,left:r.left,right:r.right,width:r.width,height:r.height,clientHeight:n.clientHeight,scrollHeight:n.scrollHeight};};const dock=document.querySelector('[data-testid="session-dock"]');return {viewport:{width:innerWidth,height:innerHeight},dock:rect('[data-testid="session-dock"]'),composerArea:rect('[data-testid="composer-area"]'),composer:rect('[data-testid="composer-surface"]'),question:rect('[data-testid="pending-question-panel"]'),stream:rect('[data-testid="stream-container"]'),list:rect('[data-testid="session-files-list"]'),toolbar:rect('.tc-composer__bar'),radius:dock?getComputedStyle(dock).borderRadius:null,horizontalOverflow:document.documentElement.scrollWidth>innerWidth};});
      assert.equal(geometry.horizontalOverflow,false);if(geometry.dock){assert.equal(geometry.radius,"4px");assert.ok(Math.abs(geometry.dock.left-geometry.composer.left)<=1);assert.ok(Math.abs(geometry.dock.right-geometry.composer.right)<=1);assert.ok(Math.abs(geometry.composerArea.top-geometry.dock.bottom-4)<=1, JSON.stringify(geometry));}
      if(geometry.question && geometry.dock){assert.ok(geometry.question.bottom<=geometry.dock.top+1);assert.ok(geometry.question.height>20,"Question panel must not be crushed by the dock/footer");}
      if(scenario.height>=844)assert.ok(geometry.composer.bottom<=scenario.height+1);
      if(scenario.height<400){
        await page.getByTestId("approval-skip").scrollIntoViewIfNeeded();
        await page.getByTestId("approval-skip").focus();
        const questionControl=await page.getByTestId("approval-skip").boundingBox();
        assert.ok(questionControl.y>=0&&questionControl.y+questionControl.height<=scenario.height+1,"Question actions must be truly reachable");
        await capture("question-accessible");
        await page.getByTestId("stop-button").scrollIntoViewIfNeeded();const rect=await page.getByTestId("stop-button").boundingBox();assert.ok(rect.y>=0 && rect.y+rect.height<=scenario.height+1);
      }
      if(scenario.menu){
        await page.getByTestId("model-select").click();
        const bounds=await page.getByTestId("model-dropdown").boundingBox();
        assert.ok(bounds.x>=0&&bounds.x+bounds.width<=scenario.width+1&&bounds.y>=0&&bounds.y+bounds.height<=scenario.height+1,"Model menu must fit narrow sidebar viewport");
        await capture("model-dropdown");await page.keyboard.press("Escape");
      }
      await capture("expanded");results.push({scenario:scenario.name,geometry,passed:true});
      if(scenario.name==="busy-both"){
        await page.getByRole("button",{name:"Expand todos"}).click();
        await page.evaluate(()=>{window.__state.sessionViews.s1.busy=false;window.__emit();});
        await page.getByTestId("session-files-list").waitFor();
        await page.evaluate(()=>{window.__state.sessionViews.s1.busy=true;window.__state.sessionViews.s1.sessionTodos=[{id:"next",content:"Answer follow-up",status:"in_progress"}];window.__emit();});
        assert.equal(await page.getByTestId("files-toggle").getAttribute("aria-expanded"),"true");
        assert.equal(await page.getByTestId("session-files-list").isVisible(),true);
        await capture("independent-todos-files");results.push({scenario:"independent-todos-files",passed:true});
      }
      if(scenario.name==="long-files"){
        await page.evaluate(()=>{window.__editor=document.querySelector('[data-testid="composer-input"]');window.__list=document.querySelector('[data-testid="session-files-list"]');window.__list.scrollTop=40;window.__state.sessionViews.s1.busy=true;window.__state.sessionViews.s1.sessionTodos=[{id:"t",content:"Answer question",status:"in_progress"}];window.__emit();});
        await page.getByTestId("files-toggle").waitFor();assert.equal(await page.getByTestId("files-toggle").getAttribute("aria-expanded"),"true");
        assert.equal(await page.evaluate(()=>window.__editor===document.querySelector('[data-testid="composer-input"]') && window.__list===document.querySelector('[data-testid="session-files-list"]') && window.__list.scrollTop===40),true);
        await page.evaluate(()=>{window.__state.sessionViews.s1.busy=false;window.__emit();});
        await page.getByTestId("files-toggle").waitFor();
        await page.getByTestId("stream-container").evaluate(stream=>{stream.scrollTop=stream.scrollHeight*.4;stream.dispatchEvent(new Event("scroll"));});
        await page.evaluate(()=>{const stream=document.querySelector('[data-testid="stream-container"]');const r=stream.getBoundingClientRect();window.__anchor=[...stream.querySelectorAll('[data-message-id]')].find(n=>{const b=n.getBoundingClientRect();return b.top>=r.top&&b.bottom<=r.bottom;})?.dataset.messageId;});
        await page.getByTestId("files-toggle").click();
        assert.ok(await page.evaluate(()=>{const stream=document.querySelector('[data-testid="stream-container"]');const anchor=stream.querySelector(`[data-message-id="${window.__anchor}"]`);if(!anchor)return false;const a=anchor.getBoundingClientRect(),s=stream.getBoundingClientRect();return a.bottom>s.top&&a.top<s.bottom;}),"Manual-reading anchor remains visible when dock collapses");
        await page.getByTestId("files-toggle").click();
        await page.evaluate(()=>{window.__state.sessionViews.s1.sessionFiles={sourceTurnId:"u4",files:[{path:"/workspace/new.ts",status:"added",added:3,removed:0,restorable:true}]};window.__emit();});
        await page.getByText("new.ts",{exact:true}).waitFor();assert.equal(await page.getByTestId("session-file-row").count(),1);assert.equal(await page.getByTestId("session-files-list").evaluate(list=>list.scrollTop),0);
        await page.getByTestId("undo-all-files").click();await page.getByRole("button",{name:/Undo File/}).click();await page.getByTestId("files-dock").waitFor({state:"detached"});
        await page.evaluate(()=>window.__emit());assert.equal(await page.getByTestId("files-dock").count(),0);assert.equal(await page.evaluate(()=>window.__editor===document.querySelector('[data-testid="composer-input"]')),true);
        await capture("undo-empty");results.push({scenario:"question-retention-switch-and-undo",passed:true});
      }
      if(["wide-files","narrow-files","narrow-light"].includes(scenario.name)) {
        await page.getByTestId("keep-all-files").click();
        assert.equal(await page.getByRole("dialog").count(),0,"Keep must not ask for confirmation");
        await page.getByTestId("files-dock").waitFor({state:"detached"});
        await capture("kept-empty");
      }
      assert.equal(consoleEvents.filter(e=>e.level==="error").length,0,JSON.stringify(consoleEvents));
    }catch(error){await capture("failed");throw error;}finally{await page.close();await writeFile(path.join(out,"geometry.json"),JSON.stringify(results,null,2));}
  }
  console.log(`PASS: ${results.length} scenarios; artifacts ${out}`);
}finally{await browser?.close();if(vite.exitCode===null){const done=new Promise(resolve=>vite.once("exit",resolve));vite.kill("SIGTERM");await done;}}
