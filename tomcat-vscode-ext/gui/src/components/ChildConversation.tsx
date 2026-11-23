import {useT} from '../i18n/LocaleProvider';
import type {ChildTask} from '../../../src/shared/backgroundTasks';
import type {WebviewSessionSnapshot} from '../types';
import {TranscriptView} from './TranscriptView';
import { roleLabel, stateLabel } from './BackgroundTasks';
const noop=()=>{};
export function ChildConversation({row,view,pending,back,older,openFile}:{row:ChildTask;view?:WebviewSessionSnapshot;pending:boolean;back():void;older():void;openFile(path:string):void}) {
 const t=useT();
 return <main className="tc-shell" data-testid="readonly-child-view">
   <header className="tc-child-header"><button data-testid="child-back" type="button" autoFocus onClick={back}>← {t('child.parent')}{pending?' 🔴':''}</button><strong title={row.description}>{row.description}</strong><small>{roleLabel(row.type)} · {stateLabel(row.state)}</small></header>
   <section className="tc-stream">{view?.hasMoreHistory && <button onClick={older}>{t('child.older')}</button>}
     {view ? <TranscriptView readOnly busy={row.state==='running'} timeline={view.timeline.filter(i=>i.type!=='approval' && i.type!=='plan' && i.type!=='checkpoint')} canBuildPlan={false} onOpenFile={openFile} onOpenPlanFile={openFile} onAnswer={noop} onBuildPlan={noop} onApprovalDraftChange={noop} onSetBuildModel={noop} onSelectSpeed={noop} onSelectContextWindow={noop} onSelectThinkingLevel={noop}/> : <p role="status">{t('chat.loadingHistory')}</p>}
   </section><footer>{t('child.readOnly')}</footer>
 </main>;
}
