import z from '@deepseek-ai/schemastery';
import { createRequire } from 'node:module';
import path from 'node:path';
import { install, inject as adapterInject } from './index.js';
export { name } from './index.js';
export const inject = [...adapterInject, 'workingDirectory'];
const absolutePath = /^(?:$|(?:[A-Za-z]:[\\/]|\/|\\\\).*)$/;
const defaultDataDir = process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, 'Retenixa') : '';
export const Config = z.object({
  command: z.string().pattern(absolutePath).default('').description('Retenixa 可执行文件的绝对路径').volatile(),
  dataDir: z.string().pattern(absolutePath).default(defaultDataDir).description('共享数据目录的绝对路径；默认为 %LOCALAPPDATA%/Retenixa，需与其他客户端共享记忆时保持一致').volatile(),
});
export const usage = '在下方设置 Retenixa 可执行文件；数据目录默认为 %LOCALAPPDATA%/Retenixa，只在与其他客户端共享记忆时才需改动。项目目录自动跟随各会话的当前工作目录，无需填写工作区路径。保存后重新连接已打开的会话；同一作用域只保留一份 Retenixa 接入。';
export async function apply(ctx, config) {
  const requireHost = createRequire(import.meta.url);
  let mounted, accepted, disposed = false, serial = Promise.resolve();
  async function reconcile() {
    if (disposed) return;
    const values = {command: config.command.get(), dataDir: config.dataDir.get()};
    const signature = JSON.stringify(values);
    if (signature === accepted) return;
    if (mounted) { await mounted.dispose(); mounted = undefined; }
    accepted = signature;
    if (!values.command || !values.dataDir) return;
    mounted = ctx.plugin({
      name: 'retenixa-session-tools',
      inject: adapterInject,
      async apply(scope) { await install(scope, values, requireHost); },
    });
    await mounted;
  }
  const schedule = () => {
    serial = serial.then(reconcile).catch(error => ctx.logger.error(error));
    return serial;
  };
  ctx.on('loader/volatile-update', schedule);
  ctx.effect(() => async () => { disposed = true; await serial; if (mounted) await mounted.dispose(); });
  ctx.inject(['settings'], child => child.effect(() => child.settings.configure({auto: false}, ctx.fiber)));
  await schedule();
}
