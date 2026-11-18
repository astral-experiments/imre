"""Normalize direct supervised finite captures into the shared checked finite core."""
import hashlib
from functools import lru_cache
from pathlib import Path
import raw_control as raw


class DirectCapture:
    def __init__(self, api, binding, data, obj):
        self.api, self.binding, self.data, self.raw = api, binding, data, obj

    def held(self, record):
        self.api.require(record['fstat_before_read']==record['fstat_after_read']==record['path_stat'], 'gate held FD changed during read or differs from path')
        return {'fd_realpath':record['canonical_path'],'path':record['path'],'fstat':record['fstat_before_read'],'sha256':record['sha256']}

    @lru_cache(maxsize=None)
    def object(self,role):
        value=self.raw(role);check=self.api.require
        if role=='stock_meta':
            check(value['same_ordered_originals_and_outcomes'] and value['stdout_identical'] and value['stderr_identical'] and self.data('stock_stdout')==self.data('prior_stock_stdout') and self.data('stock_stderr')==self.data('prior_stock_stderr') and self.data('stock_effects_before')==self.data('stock_effects_after') and value['timeout_policy']=='absolute-monotonic-from-before-popen' and value['timeout_seconds']==240, 'stock capture differs from frozen observations or absolute bound')
            return {**value,'original_stock_metadata_sha256':value['original_metadata_sha256'],'ordered_streams_and_effects_equal':True}
        if role in {'O0_meta','O3_meta'}:
            t=role[:2];image=value['argv'][-1];trace=self.data(t+'_trace').decode();first,child=raw.execs(trace.splitlines(),check);env=raw.exec_environment(trace.splitlines()[child[0]],check);pre=value['held_fd_pre'];post=value['held_fd_post'];sample=value['proc_exe_sample'];check(value['emergency_restores']==[] and pre==post and pre['fstat']==value['path_post_fstat']==sample['fstat'] and sample['pid']==child[1] and sample['target']==image==child[2] and value['same_ordered_originals_and_outcomes'] and value['timeout_seconds']==240 and value['timeout_policy']=='absolute-monotonic-from-before-popen', 'native identity, restoration or bound differs')
            return {**value,'argv':[image],'capture_argv':value['argv'],'environment':env,'supervisor_environment':value['environment'],'before':{image:pre['sha256']},'after':{image:post['sha256']},'image_sha256':pre['sha256'],'supervisor_sha256':self.binding['probe']['executable_sha256'],'restored':True,'image_identity_before_after':True}
        if role in {'O0_build_json','O3_build_json'}:
            return {**value,'argv':value['build_argv'],'exit_code':value['compiler_exit_code'],'image_sha256':value['new_image_sha256']}
        if role.startswith('gate_') and role.endswith('_meta'):
            _,t,mode,_=role.split('_');positive=self.raw(t+'_build_json');trace=self.data('gate_'+t+'_'+mode+'_trace').decode();first,child=raw.execs(trace.splitlines(),check);sample=value['live_compiler_sample'];held=value['held_before']['cli'];check(value['held_before']==value['held_after'] and value['compiler_child_exit_code']==raw.child_exit_code(trace.splitlines(),child[1],check)==1 and sample['pid']==int(child[1]) and sample['target']==child[2]==held['canonical_path'] and sample['fstat']==held['fstat_before_read'] and value['start_utc_ns']<sample['sample_utc_ns']<value['finish_utc_ns'] and value['negative_launcher_sha256']==raw.sha(self.data(mode+'_launcher')), 'live gate compiler, source or exit differs')
            return {**value,'original_launcher_sha256':raw.sha(self.data('positive_launcher')),'positive_build_metadata_sha256':raw.sha(self.data(t+'_build_json')),'positive_build_argv':positive['build_argv'],'build_argv':child[3],'probe_sha256':self.binding['probe']['executable_sha256'],'probe_source_sha256':self.binding['probe']['source_sha256'],'supervisor_exit_code':value['exit_code'],'actual_compiler_child_environment':raw.exec_environment(trace.splitlines()[child[0]],check),'poison_path':str(Path(child[3][2]).parent/(self.binding['poison_module']+'.py')) if mode=='imported' else None}
        return value

    @lru_cache(maxsize=None)
    def identity_object(self,role):
        if role in {'O0_build_json','O3_build_json'}:return self.raw(role)
        if role=='gate_bracket':
            b=self.raw(role);check=self.api.require;check(b['held_before']==b['held_after'] and b['gates']==[t+'.'+m for t in ('O0','O3') for m in ('direct','imported')], 'global gate bracket or census differs');return {'fd_before':{k:self.held(v) for k,v in b['held_before'].items()},'fd_after':{k:self.held(v) for k,v in b['held_after'].items()},'fd_unchanged':True,'controls':[{'tier':t,'kind':m,'record_sha256':raw.sha(self.data('gate_'+t+'_'+m+'_meta'))} for t in ('O0','O3') for m in ('direct','imported')],'controller_sha256':b['controller_sha256']}
        if role.startswith('gate_') and role.endswith('_meta'):
            _,t,mode,_=role.split('_');v=self.object(role);old=self.raw('prior_'+role);check=self.api.require;check(self.api.comparable_build(old['target_argv'][3:])==self.api.comparable_build(v['build_argv']) and old['held_before']==old['held_after'] and all(old['held_before'][k]['sha256']==v['held_before'][k]['sha256'] for k in ('cli','runtime','clang')), 'gate predecessor graph or products differ');fd={k:x['fstat_before_read'] for k,x in v['held_before'].items()};post={k:x['fstat_after_read'] for k,x in v['held_after'].items()};return {**v,'actual_compiler_path':v['build_argv'][0],'actual_compiler_argv':v['build_argv'],'actual_compiler_environment':v['actual_compiler_child_environment'],'source_negative_sha256':v['negative_launcher_sha256'],'fd_fstat_before':fd,'fd_fstat_after':post,'corrected_gate_record_sha256':raw.sha(self.data('prior_'+role))}
        if role.startswith('prior_') and role.endswith('_meta'):
            v=self.raw(role);return {**v,'argv':[v['target_argv'][-1]]}
        return self.raw(role)

    def context(self,entry=None):
        b=self.binding;check=self.api.require;source=self.data('stock_source');blob=hashlib.sha1(f'blob {len(source)}\0'.encode()+source).hexdigest();a=self.raw('assessment')
        check(raw.sha(source)==b['source']['sha256'] and blob==b['source']['git_blob'] and a['source']==b['source'] and a['revision']==b['revision']==self.api.COHORT['imre_revision'] and {k:a['products'][k] for k in ('cli','runtime','clang')}==b['product'] and a['products']['stock']==b['stock_interpreter']['sha256'] and raw.sha(self.data('gate_probe_source'))==b['probe']['source_sha256'] and raw.sha(self.data('gate_probe_binary'))==b['probe']['executable_sha256'], 'direct source, assessment, product or probe identity differs')
        custody=self.raw('post_custody');before=self.raw('capture_before');check(custody['controller_sha256']==raw.sha(self.data('stock_controller')) and custody['baseline_sha256']==raw.sha(self.data('capture_before')) and custody['after']==before and before['fixture_equal'] is True and before['fixture_files']==len(self.raw('fixture_manifest')), 'post-invocation fixture, source, product or image custody differs')
        if entry is not None:
            authority=self.api.extension_authority(entry);check((b['stock_interpreter']['sha256'],b['stock_interpreter']['version'])==authority.predecessor_stock(self.api),'direct stock differs from predecessor');authority.donor(self.api,b['product'])
        run={'family':b['family'],'revision':b['revision'],'runs':{t:self.object(t+'_meta') for t in self.api.TIERS}}
        return {'legacy':False,'run':run,'platform':dict(self.api.COHORT['platform']),'gate_tiers':('O0','O3'),'poison_module':b['poison_module'],'operations':b['operations'],'probe':b['probe'],'observed':len(b['originals']),'withdrawal':b['withdrawal'],'stock_interpreter':b['stock_interpreter'],'identity':{'held_proof':'read-bracket-v1'},'roles':b['roles'],'join_modes':b['join_modes'],'raw_obj':self.raw,'identity_obj':self.identity_object,'product':b['product'],'source':b['source'],'completion':{'schema':'imre.compatibility.completed-unittest.v1'},'ordered_originals':[(x['id'],x['occurrence']) for x in b['originals']],'tier_environment':{'schema':'imre.compatibility.finite-tier-environment.v1','tiers':{'O0':'stock-unchanged','O3':'stock-unchanged'}},'build_controller_roles':{'O0':'O0_build_controller','O3':'O3_build_controller'}}
