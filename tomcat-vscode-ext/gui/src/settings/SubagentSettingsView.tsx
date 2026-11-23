import {useEffect} from 'react';
import type {SettingsStateSnapshot,SettingsIntent,VsCodeApiLike} from '../../../src/shared/settingsProtocol';
import {useT} from '../i18n/LocaleProvider';
import {ModelRolePicker} from './ModelRolePicker';
import type {ProviderCard} from './providerCards';
import {SettingsShell} from './SettingsShell';
import {send} from './settingsShared';

export function SubagentSettingsView({state,cards,vscodeApi}:{state:SettingsStateSnapshot;cards:ProviderCard[];vscodeApi:VsCodeApiLike<SettingsIntent>}) {
  const t=useT();
  useEffect(()=>{
    if (state.focusRole) document.getElementById(`subagent-${state.focusRole.role}`)?.focus();
  },[state.focusRole?.role,state.focusRole?.revision]);
  return <SettingsShell state={state} vscodeApi={vscodeApi}><main className="tc-settings-shell__content">
    <header className="tc-settings-shell__header"><h1>{t('settings.subagents')}</h1></header>
    {state.error && <p role="alert">{state.error}</p>}{state.warnings?.map(w=><p key={w} role="alert">{w}</p>)}{state.status && <p role="status">{state.status}</p>}
    {(['explorer','plan_reviewer'] as const).map(role=>{const locked=role==='plan_reviewer' && state.planReviewerModelLocked;
      return <section className="tc-settings-group" key={role} id={`subagent-${role}`} tabIndex={-1}>
        <h2>{role==='explorer'?'Explorer':'Plan Reviewer'}</h2><p>{t(`subagent.${role}`)}</p>
        <ModelRolePicker role={role} canSet={!!state.ready && !!state.subagentModels && !!state.capabilities.updateSubagentSettings && !locked} cards={cards} models={state.models} modelId={state.subagentModels?.[role] ?? ''} onSelect={id=>send(vscodeApi,{type:'updateSubagentSettings',data:{role,model:id || null}})}/>
        {locked && <p>{t('subagent.envLocked')}</p>}
      </section>;
    })}
  </main></SettingsShell>;
}
