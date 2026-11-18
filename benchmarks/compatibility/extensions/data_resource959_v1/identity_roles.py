"""Normalize a held-FD, supervised finite capture into the shared finite protocol."""
from __future__ import annotations
import datetime,hashlib,json
from functools import lru_cache
from pathlib import Path
import raw_control as raw

def utc_ns(value):
    dt=datetime.datetime.fromisoformat(value);epoch=datetime.datetime(1970,1,1,tzinfo=datetime.timezone.utc);delta=dt.astimezone(datetime.timezone.utc)-epoch
    return ((delta.days*86400+delta.seconds)*1000000+delta.microseconds)*1000

class HeldCapture:
    def __init__(self, api, binding, data, raw_obj):
        self.api,self.binding,self.data,self.raw_obj=api,binding,data,raw_obj
        self.identity=binding['identity'];self.exec_policy=self.identity['exec_policy']
        api.require(self.identity['capture_format']=='held-supervised-v1', 'unknown finite capture format')

    def bracket(self,tier):
        b=self.raw_obj(tier+'_gate_bracket');pre=b['fd_before'] if 'fd_before' in b else self.raw_obj(tier+'_gate_fd_before');post=b['fd_after'] if 'fd_after' in b else self.raw_obj(tier+'_gate_fd_after');controls=[]
        for x in b['controls']:
            controls.append({'tier':tier,'kind':x['mode'],'record_sha256':raw.sha(self.data('gate_'+tier+'_'+x['mode']+'_meta'))})
        return {'fd_before':pre,'fd_after':post,'fd_unchanged':b.get('stable',b.get('fd_records_identical')),'controls':controls,'controller_sha256':b['controller_sha256']}

    @lru_cache(maxsize=None)
    def object(self,role):
        value=self.raw_obj(role)
        if role=='stock_meta':
            prior=self.raw_obj('prior_stock_meta');c=self.data('stock_controller')
            self.api.require(value['timeout_policy']=='absolute-monotonic-from-before-popen' and value['identity_sample_covered_by_timeout'] is True and value['timeout_seconds']==prior['timeout_seconds']==240 and value['observed_elapsed_seconds']<value['timeout_seconds'] and b'deadline-time.monotonic()' in c and b'time.monotonic()<deadline' in c, 'stock absolute timeout or sample policy differs')
            return {**value,'timed_out':False}
        if role in {'O0_meta','O3_meta'}:
            tier=role[:2];capture=value;before=self.raw_obj(tier+'_source_before');after=self.raw_obj(tier+'_source_after');cleanup=self.raw_obj(tier+'_cleanup');image=capture['argv'][-1];chain=self.exec_policy['tiers'][tier];args=capture['argv'];i=args.index(chain['probe']);self.api.require(args[i+1:i+3]==['withdraw-wrap',str(chain['timeout_seconds'])], 'native timeout differs from declared supervisor')
            return {**capture,'argv':[image],'capture_argv':args,'timeout_seconds':chain['timeout_seconds'],'timed_out':cleanup['timed_out'],'before':{image:before['image']},'after':{image:after['image']},'restored':before==after and cleanup['restoration']['restored'],'image_sha256':before['image'],'supervisor_sha256':capture['probe_sha256'],'controller_sha256':capture['source_controller_sha256'],'image_identity_before_after':before['image']==after['image'],'effects_before_sha256':raw.sha(self.data(tier+'_effects_before')),'effects_after_sha256':raw.sha(self.data(tier+'_effects_after'))}
        if role in {'O0_build_json','O3_build_json'}:
            baseline=self.raw_obj('capture_before')['inputs'];argv=value['target_argv'];launcher=argv[2];source=str(Path(self.api.option(argv,'--extension-path'))/Path(self.binding['source']['path']).relative_to('Lib'))
            return {**value,'argv':argv,'source_sha256':baseline[source]['sha256'],'launcher_sha256':baseline[launcher]['sha256']}
        if role.startswith('gate_') and role.endswith('_meta'):
            _,tier,mode,_=role.split('_');br=self.raw_obj(tier+'_gate_bracket');records=[x for x in br['controls'] if x['mode']==mode];self.api.require(len(records)==1 and records[0]==value, 'gate record is not the held bracket member');positive=self.raw_obj(tier+'_build_json');trace=self.data('gate_'+tier+'_'+mode+'_trace').decode();first,child=raw.execs(trace.splitlines(),self.api.require);gate=self.binding['cohorts'][self.binding['cohort']]['gate_probe'];poison=str(Path(positive['target_argv'][2]).parent/(self.binding['cohorts'][self.binding['cohort']]['poison_module']+'.py')) if mode=='imported' else None
            self.api.require(raw.child_exit_code(trace.splitlines(),child[1],self.api.require)==value.get('compiler_exit'), 'gate recorded child exit differs from actual trace');negative=value.get('negative_launcher_sha256',value.get('negative_sha256'));original=value.get('positive_launcher_sha256',value.get('positive_sha256'));env=value.get('environment',value.get('compiler_environment'));self.api.require((value.get('positive_metadata_sha256')==raw.sha(self.data(tier+'_build_json')) if 'positive_metadata_sha256' in value else br['image_sha256']==positive['image_sha256']), 'gate positive graph identity differs')
            return {**value,'negative_launcher_sha256':negative,'original_launcher_sha256':original,'positive_build_metadata_sha256':raw.sha(self.data(tier+'_build_json')),'positive_build_argv':positive['target_argv'],'build_argv':child[3],'probe_sha256':gate['executable_sha256'],'probe_source_sha256':gate['source_sha256'],'supervisor_exit_code':value.get('supervisor_exit'),'compiler_child_exit_code':raw.child_exit_code(trace.splitlines(),child[1],self.api.require),'actual_compiler_child_environment':env,'poison_path':poison,'image_exists':value.get('image_exists')}
        return value

    @lru_cache(maxsize=None)
    def identity_object(self,role):
        if role in {'O0_build_json','O3_build_json'}:
            tier=role[:2];v=self.raw_obj(role);base=self.raw_obj('capture_before')['inputs'];before=v['held_fd_before'];after=v['held_fd_after'];trace=self.data(tier+'_positive_trace').decode().splitlines();events=raw.successful_exec_events(trace,self.api.require);matches=[e for e in events if e[2]==v['target_argv'][0]];self.api.require(len(matches)==1 and v['held_fd_before']==v['held_fd_after'] and v['fd_records_identical'] is True, 'positive held product or compiler event differs');e=matches[0];prior=self.raw_obj('prior_'+role);normalized=self.object(role)
            return {**v,'build_argv':v['target_argv'],'fd_before':before,'fd_after':after,'compiler_pid':int(e[1]),'actual_compiler_path':e[2],'actual_compiler_argv':e[3],'actual_compiler_environment':raw.exec_environment(e[4],self.api.require),'controller_sha256':v['capture_controller_sha256'],'compiler_exit_code':raw.child_exit_code(trace,e[1],self.api.require),'strace_exit_code':v['exit_code'],'timed_out':v['outer_timeout'],'start_utc_ns':utc_ns(v['utc_start']),'finish_utc_ns':utc_ns(v['utc_finish']),'positive_build_metadata_sha256':raw.sha(self.data('prior_'+role)),'source_sha256':normalized['source_sha256'],'launcher_sha256':normalized['launcher_sha256'],'new_image_sha256':v['image_sha256'],'old_image_sha256':v['original_image_sha256'],'elf_identical':v['image_matches_original'],'effects_before_sha256':raw.sha(self.data(tier+'_positive_effects_before')),'effects_after_sha256':raw.sha(self.data(tier+'_positive_effects_after')),'prior_image_matches':prior['image_sha256']==v['original_image_sha256']}
        if role.endswith('_gate_bracket'):return self.bracket(role[:2])
        if role.startswith('gate_') and role.endswith('_meta'):
            _,tier,mode,_=role.split('_');v=self.object(role);br=self.bracket(tier);return {**v,'controller_sha256':br['controller_sha256'],'actual_compiler_path':v['build_argv'][0],'actual_compiler_argv':v['build_argv'],'actual_compiler_environment':v['actual_compiler_child_environment'],'source_negative_sha256':v['negative_launcher_sha256']}
        if role.startswith('prior_') and role.endswith('_meta'):
            v=self.raw_obj(role);return {**v,'capture_argv':v['argv'],'argv':[v['argv'][-1]]}
        return self.raw_obj(role)

    def run(self):
        return {'family':self.binding['family'],'revision':self.api.COHORT['imre_revision'],'runs':{t:self.object(t+'_meta') for t in self.api.TIERS}}

    def context(self,entry,cohort):
        api=self.api;check=api.require;b=self.binding;a=self.raw_obj('assessment');source=b['source'];originals=self.raw_obj('originals');product={'cli':a['products']['imre'],'runtime':a['products']['runtime'],'clang':a['products']['clang']};authority=api.extension_authority(entry);identity=b['stock_interpreter'];expected=authority.predecessor_stock(api);blob=self.data('stock_source');git_blob=hashlib.sha1(f'blob {len(blob)}\0'.encode()+blob).hexdigest()
        check(a['revision']==api.COHORT['imre_revision'] and a['source']==source and raw.sha(blob)==source['sha256'] and git_blob==source['git_blob'] and a['inventory_count']==b['observed_originals']==len(originals) and a['inventory_sha256']==raw.sha(self.data('originals')) and originals==[{k:r[k] for k in ('id','occurrence')} for r in b['originals']] and a['launcher_sha256']==raw.sha(self.data('positive_launcher')) and a['stock_sha256']==identity['sha256']==expected[0] and identity['version']==expected[1] and a['stock_version'].startswith(identity['version']+' '), 'finite immutable source, original inventory, launcher or stock identity differs')
        authority.donor(api,entry,product);probe=cohort['gate_probe'];check(probe['source_sha256']==raw.sha(self.data('gate_probe_source')) and probe['executable_sha256']==raw.sha(self.data('gate_probe_binary')) and cohort['source_leaf']==source['path'] and cohort['strict_source_control']=='native-content-v1', 'finite first-party gate probe or source leaf differs')
        return {'legacy':False,'run':self.run(),'platform':dict(api.COHORT['platform']),'gate_tiers':('O0','O3'),'poison_module':cohort['poison_module'],'operations':cohort['gate_operations'],'probe':probe,'observed':b['observed_originals'],'withdrawal':b['withdrawal'],'stock_interpreter':identity,'identity':b['identity'],'roles':b['roles'],'join_modes':b['identity']['join_modes'],'raw_obj':self.raw_obj,'identity_obj':self.identity_object,'product':product,'source':source,'exec_policy':b['identity']['exec_policy'],'resource_roles':b['resource_roles'],'tier_environment':b['tier_environment'],'build_exec_policy':b['build_exec_policy'],'ordered_originals':[(x['id'],x['occurrence']) for x in originals],'elf_tool_sha256':b['elf_tool_sha256']}
