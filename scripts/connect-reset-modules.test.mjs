import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,readFileSync,writeFileSync,existsSync,rmSync,statSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {connect,connectionOptions} from './connect-reset-modules.mjs';

test('runtime connection requires an explicit complete configuration',()=>{
  const exports=['--forecast','/tmp/forecast.json','--evaluation','/tmp/report.json'];
  assert.equal(connectionOptions(exports).managed,false);
  assert.throws(()=>connectionOptions([...exports,'--python','/tmp/python']),/requires/);
  assert.throws(()=>connectionOptions([...exports,'--python','/tmp/python','--observer-dir','/tmp/module','--observer-db','/tmp/data','--external']),/Choose/);
  assert.throws(()=>connectionOptions([...exports,'--shell','anything']),/Invalid option/);
});
test('failed connection preserves the existing configuration and external mode only detaches supervision',()=>{
  const root=mkdtempSync(join(tmpdir(),'horizon-connect-'));
  try {
    const forecast=join(root,'forecast.json'),report=join(root,'report.json');
    writeFileSync(forecast,JSON.stringify({schema_version:'reset-intelligence-consumer/1'}));
    writeFileSync(report,JSON.stringify({schema_version:'reset-observer/1'}));
    const args=['--forecast',forecast,'--evaluation',report,'--data-dir',root];
    connect(args);
    const path=join(root,'reset-radar-connection.json'),before=readFileSync(path);
    if(process.platform!=='win32') assert.equal(statSync(path).mode & 0o777,0o600);
    writeFileSync(report,'{}');
    assert.throws(()=>connect(args),/Expected/);
    assert.deepEqual(readFileSync(path),before);
    writeFileSync(report,JSON.stringify({schema_version:'reset-observer/1'}));
    writeFileSync(join(root,'reset-radar-runtime.json'),'{}');
    connect([...args,'--external']);
    assert.equal(existsSync(join(root,'reset-radar-runtime.json')),false);
    assert.equal(existsSync(report),true);
  } finally {rmSync(root,{recursive:true});}
});
