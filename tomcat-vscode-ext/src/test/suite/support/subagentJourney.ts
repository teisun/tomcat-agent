import * as assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import type { TomcatExtensionApi } from '../../../extension';
import { TEST_DEFAULT_CWD_ENV } from '../../../constants';
import { spawnScriptedOpenAiStreamServer, sseDelta, sseDone, sseFinish, sseToolCall } from './scriptedModel';
import { SettingsFrameDriver } from './workbenchFindDriver';

function gate() {
  let release!: () => void;
  const waitFor = new Promise<void>(r => { release = r; });
  return { waitFor, release };
}
async function until(check: () => boolean | Promise<boolean>, label: string) {
  const end = Date.now() + 25_000;
  do { if (await check()) return; await new Promise(r => setTimeout(r, 70)); } while (Date.now() < end);
  throw new Error(`Timed out: ${label}`);
}

/** Only the provider is scripted. Responses to simultaneous Explorer calls are routed by brief, not arrival order. */
export async function assertSubagentJourney(api: TomcatExtensionApi) {
  const cwd = process.env[TEST_DEFAULT_CWD_ENV];
  const artifacts = process.env.TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR;
  const port = Number(process.env.TOMCAT_E2E_OBSERVATION_MODEL_PORT);
  assert.ok(cwd && artifacts && port);
  const holds = [gate(), gate(), gate()];
  const names = ['Map project entry points', 'Check settings', 'Trace the session router from request dispatch to background report delivery'];
  const prefixes = ['Inspecting entry points.', 'Inspecting settings.', 'Inspecting routing.'];
  const calls: string[] = [];
  const mainRequests: string[] = [];
  const task = (i: number) => ({ type: 'explorer', description: names[i], prompt: `Independently investigate fixture-area-${i}.` });
  const tool = (id:string, name:string, args:unknown) => ({ parts:[sseToolCall(id,name,JSON.stringify(args)),sseFinish('tool_calls'),sseDone()] });
  const text = (s:string) => ({ parts:[sseDelta(s),sseFinish('stop'),sseDone()] });
  const model = await spawnScriptedOpenAiStreamServer(body => {
    const req = JSON.parse(body);
    if (!req.tools?.length) return text('Workspace check');
    if (String(req.messages[0].content).startsWith('You are an Explorer subagent')) {
      const i = names.findIndex((_,i)=>body.includes(`fixture-area-${i}`));
      calls.push(`explorer:${i}`);
      return {parts:[sseDelta(prefixes[i]),{body:'',waitFor:holds[i].waitFor},sseDelta(' Verified routing finding.'),sseFinish('stop'),sseDone()]};
    }
    mainRequests.push(body);
    switch(mainRequests.length) {
      case 1:return tool('default-wait','dispatch_agent',{tasks:[task(0)]});
      case 2:return text('Foreground investigation closed.');
      case 3:return tool('parallel','dispatch_agent',{tasks:[task(1),task(2)],run_in_background:true});
      case 4:return tool('list','subagent_list',{});
      case 5:return tool('background-shell','bash',{command:'echo $$ > worker.pid; exec sleep 120',run_in_background:true});
      case 6:return text('Background checks are running.');
      case 7:return text('Combined routing report.');
      default:return {status:400,parts:[{body:'unexpected parent turn'}]};
    }
  }, [], port);
  let chat: SettingsFrameDriver | undefined;
  try {
    const host=api.__testing;
    await host.focusWebview(); await host.waitForWebviewReady();
    await until(()=>!!host.getWebviewState().activeSessionId && host.getWebviewState().ready,'parent initialized');
    const parent=host.getWebviewState().activeSessionId!;
    const state=()=>host.getWebviewState().sessionViews[parent];
    const rows=()=>state().backgroundTasks ?? [];
    chat=await SettingsFrameDriver.connectFromEnvironment('.tc-shell');
    const fillDraft=async(text:string)=>{
      await until(()=>chat!.evaluate<boolean>(`document.querySelector('[data-testid="composer-input"]')?.isContentEditable === true`),'editable composer rendered');
      await host.sendWebviewDomAction({kind:'setInputValue',testId:'composer-input',value:text});
    };
    const send=async(prompt:string)=>{await fillDraft(prompt);await chat!.click('send-button');};
    await send('Check the project entry points.');
    await until(()=>calls.includes('explorer:0'),'foreground child in provider');
    await chat.hover('card-child-title');
    await chat.waitForSnapshot(s=>s.buttons.some(b=>b.testId==='card-stop-subagent' && b.name==='Stop'),'foreground Stop label');
    await chat.capture(path.join(artifacts,'subagent-foreground.png'));
    await chat.click('card-stop-subagent');
    await until(()=>!state().busy && rows().some(r=>r.toolCallId==='default-wait' && r.state==='stopped'),'default waiting dispatch stopped by card');
    holds[0].release();
    await send('Investigate settings and routing in parallel.');
    await until(()=>!state().busy && calls.includes('explorer:1') && calls.includes('explorer:2') && rows().some(r=>r.kind==='shell' && r.state==='running'),'parent idle with both children and shell active');
    const children=rows().filter(r=>r.kind==='subagent' && r.toolCallId==='parallel');assert.equal(children.length,2);
    const tracked=JSON.parse(mainRequests[4]).messages.filter((m:{role:string;tool_call_id?:string})=>m.role==='tool' && m.tool_call_id==='list');
    for(const c of children)assert.ok(JSON.stringify(tracked).includes(c.id));
    const pid=Number(await fs.readFile(path.join(cwd,'worker.pid'),'utf8'));assert.ok(pid>1);process.kill(pid,0);
    await chat.click('dock-stop-shell');
    await until(()=>{try{process.kill(pid,0);return false;}catch(e){return (e as NodeJS.ErrnoException).code==='ESRCH';}},'real shell exited');
    const child=children.find(r=>r.kind==='subagent' && r.description===names[2])!;
    assert.ok(child);
    const index=async(testId:string,id:string)=>chat!.evaluate<number>(`[...document.querySelectorAll('[data-testid="${testId}"]')].findIndex(button=>button.closest('[data-task-id]')?.dataset.taskId===${JSON.stringify(id)})`);
    const other=children.find(r=>r.id!==child.id)!;
    await fillDraft('Keep my local draft');
    await host.executeCommand('workbench.action.maximizeAuxiliaryBar');
    await chat.setViewport(1440,900);
    await chat.waitForSnapshot(s=>s.viewport.width>=1000,'desktop sidebar maximized');
    await chat.hover('dock-child-title');
    const verifyRows = async () => assert.ok(await chat!.evaluate<boolean>(`[...document.querySelectorAll('[data-testid="card-child-title"], [data-testid="dock-child-title"]')].every(button => {
      const row = button.closest('li'), role = button.nextElementSibling;
      if (!row || !role || getComputedStyle(row).listStyleType !== 'disc') return false;
      const gap = role.getBoundingClientRect().left - button.getBoundingClientRect().right;
      return gap >= 0 && gap <= 8;
    })`), 'both lists indent and keep the role next to its description');
    await verifyRows();
    await until(()=>chat!.evaluate<boolean>(`[...document.querySelectorAll('[data-testid="dock-stop-subagent"]')].some(b=>b.textContent==='Stop' && getComputedStyle(b).opacity==='1')`),'visible hovered Stop');
    await chat.capture(path.join(artifacts,'subagents-running.png'));
    await chat.setViewport(390,844);
    await chat.waitForSnapshot(s=>s.viewport.width>=300 && s.viewport.width<=390,'narrow Dock');
    await verifyRows();
    await chat.hover('dock-child-title');
    await chat.capture(path.join(artifacts,'subagents-running-narrow.png'));
    await chat.setViewport(1440,900);
    await chat.click('dock-stop-subagent',await index('dock-stop-subagent',other.id));
    await until(()=>rows().some(r=>r.id===other.id && r.state==='stopped'),'sibling selectively stopped');holds[1].release();
    assert.ok(rows().some(r=>r.id===child.id && r.state==='running'));
    // The host projection updates before React removes the stopped sibling. Wait
    // for the actual surviving row rather than caching its old array index.
    await chat.waitForSnapshot(s=>s.buttons.filter(b=>b.testId==='dock-child-title').length===1 && s.buttons.some(b=>b.testId==='dock-child-title' && b.name===names[2]),'remaining child rendered in Dock');
    await chat.click('dock-child-title');
    await chat.waitForSnapshot(s=>s.text.includes(prefixes[2]),'running child replay');
    assert.equal(await chat.evaluate<boolean>(`!!document.querySelector('[data-testid="readonly-child-view"] [contenteditable=true]')`),false);
    await chat.setViewport(390,844);await chat.waitForSnapshot(s=>s.viewport.width>=300 && s.viewport.width<=390,'narrow sidebar');await chat.capture(path.join(artifacts,'subagent-readonly.png'));await chat.setViewport(1440,900);
    await chat.click('child-back');await chat.click('child-role-settings',await index('child-role-settings',child.id));
    const settings=await SettingsFrameDriver.connectFromEnvironment('#subagent-explorer');
    try {
      await until(()=>settings.evaluate<boolean>(`document.activeElement?.id==='subagent-explorer'`),'deep link focus');
      await settings.click('settings-explorer-model-toggle');await settings.click('settings-explorer-option-gpt-5.4');
      await settings.waitForSnapshot(s=>s.text.includes('Subagent settings saved.'),'role model saved');
      await settings.capture(path.join(artifacts,'subagent-models.png'));
    } finally {settings.close();}
    await host.executeCommand('workbench.action.closeActiveEditor');await host.focusWebview();
    holds[2].release();
    await until(()=>mainRequests.length>=7 && !state().busy && state().timeline.some(t=>t.type==='message' && t.text==='Combined routing report.'),'automatic report turn completed');
    assert.ok(mainRequests[6].includes('Verified routing finding.'));assert.equal(mainRequests.length,7);
    assert.equal(state().timeline.filter(t=>t.type==='message' && t.text==='Subagent reports received: 1').length,1);
    assert.equal(state().composerDraft?.text,'Keep my local draft');
    chat.close();await host.reloadWebview();await host.waitForWebviewReady();chat=await SettingsFrameDriver.connectFromEnvironment('.tc-shell');
    await until(()=>rows().some(r=>r.id===child.id && r.state==='completed') && rows().some(r=>r.id===other.id && r.state==='stopped'),'terminal rows after reload');
    await chat!.evaluate(`document.querySelector('[data-task-id="${child.id}"][data-state="completed"]')?.scrollIntoView({block:'center'})`);
    await chat.waitForSnapshot(s=>s.text.includes('Completed') && s.text.includes('Stopped'),'historical task states rendered');
    await chat.capture(path.join(artifacts,'subagents-ended.png'));
    await chat.click('card-child-title',await index('card-child-title',child.id));await chat.waitForSnapshot(s=>s.text.includes('Verified routing finding.'),'historical child transcript');
    await chat.capture(path.join(artifacts,'subagent-history.png'));
    assert.deepEqual(host.getObservedWebviewErrors(),[]);
    await fs.writeFile(path.join(artifacts,'subagent-journey.json'),JSON.stringify({parent,children:children.map(c=>c.id),shellPid:pid,parentRequests:mainRequests.length,childCalls:calls},null,2));
  } catch(error){
    const s=api.__testing.getWebviewState();
    // Fixture data only: keep projected state and the last rendered view, never HTTP headers.
    await fs.writeFile(path.join(artifacts,'journey-diagnostic.json'),JSON.stringify({calls,mainCount:mainRequests.length,session:s.activeSessionId && s.sessionViews[s.activeSessionId],dom:await chat?.snapshot().catch(()=>null)},null,2));
    throw error;
  } finally {for(const h of holds)h.release();chat?.close();await model.close();}
}
