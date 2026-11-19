window.__ModuleLoader__.load({
  id: '@retenixa/dsh-adapter',
  factory(require) {
    const React = require('react');
    const {Input, Button} = require('@deepseek-ai/dsh-client-ui-primitives');
    const h = React.createElement;
    const muted = {fontSize: 12, lineHeight: '18px', color: 'var(--dsw-alias-label-secondary)'};
    const row = {display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 16, padding: '16px 0', borderBottom: '0.5px solid var(--dsw-alias-border-l2)'};
    const absolute = value => /^(?:[A-Za-z]:[\\/]|\/|\\\\)/.test(value);
    function Settings({form}) {
      const subscribe = React.useCallback(listener => form.subscribe(listener), [form]);
      const read = React.useCallback(() => form.getSnapshot(), [form]);
      const state = React.useSyncExternalStore(subscribe, read, read);
      const [draft, setDraft] = React.useState(null);
      const [revision, setRevision] = React.useState();
      const [saving, setSaving] = React.useState(false);
      const [message, setMessage] = React.useState('');
      const base = {command: state.value?.command ?? '', dataDir: state.value?.dataDir ?? ''};
      const values = draft ?? base;
      const disabled = state.status !== 'ready' || !state.writable || saving;
      function edit(field, value) {
        if (draft === null) setRevision(state.revision);
        setDraft({...values, [field]: value});
        setMessage('');
      }
      async function save(event) {
        event.preventDefault();
        if (disabled || draft === null) return;
        if (!absolute(values.command) || !absolute(values.dataDir)) {
          setMessage('请填写可执行文件和共享数据目录的绝对路径。');
          return;
        }
        setSaving(true);
        setMessage('');
        try {
          const accepted = await form.mutate(Object.entries(values).map(([field, value]) => ({op: 'set', path: [field], value})), revision);
          if (accepted) { setDraft(null); setMessage('已保存，插件将使用新配置重新连接会话。'); }
          else setMessage('保存未被接受，配置可能已在其他页面更新。请重新载入后再保存。');
        } catch (error) {
          setMessage('保存失败：' + error.message);
        } finally { setSaving(false); }
      }
      const field = (key, title, hint, placeholder) => h('div', {key, style: row},
        h('div', {style: {flex: '1 1 240px'}},
          h('label', {htmlFor: 'retenixa-' + key, style: {fontSize: 13, fontWeight: 500, lineHeight: '20px'}}, title),
          h('div', {id: 'retenixa-' + key + '-hint', style: muted}, hint)),
        h('div', {style: {flex: '1 1 320px', minWidth: 0}}, h(Input, {
          id: 'retenixa-' + key, value: values[key], disabled,
          placeholder, 'aria-describedby': 'retenixa-' + key + '-hint',
          onChange: event => edit(key, event.target.value), style: {width: '100%'},
        })));
      return h('form', {onSubmit: save, 'aria-label': 'Retenixa 设置', style: {padding: '8px 0 24px', color: 'var(--dsw-alias-label-primary)'}},
        field('command', 'Retenixa 可执行文件', '选择已构建的 retenixa 可执行文件，所有项目共用。', 'D:/Tools/Retenixa/retenixa.exe'),
        field('dataDir', '共享数据目录', '默认为 %LOCALAPPDATA%/Retenixa；需要跨客户端共享记忆时，使用与其他 MCP 客户端相同的数据目录。', 'D:/Data/Retenixa'),
        state.status !== 'ready' ? h('p', {role: 'status', style: muted}, state.status === 'loading' ? '正在读取设置…' : '请启用下方的 Retenixa 组件后配置。') :
          !state.writable ? h('p', {style: muted}, '当前部署的设置为只读。') : null,
        h('div', {style: {display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 16}},
          h(Button, {type: 'button', disabled: disabled || draft === null, onClick: () => {setDraft(null); setMessage('');}}, '重新载入'),
          h(Button, {type: 'submit', disabled: disabled || draft === null}, saving ? '保存中…' : '保存设置')),
        message ? h('p', {role: 'status', style: muted}, message) : null);
    }
    return {
      inject: ['slots', 'configForms'],
      apply(ctx) {
        const form = ctx.configForms.get('retenixa-dsh-adapter');
        ctx.slots.inject('plugins.bundle.config', () => ctx.slots.register({
          name: 'plugins.bundle.config', key: '@retenixa/dsh-adapter',
        }, props => props?.view === 'summary' ? null : h(Settings, {form})));
      },
    };
  },
});
