import { createWallet } from '/assets/wallet.js';
import { openReceive } from '/assets/receive.js';
import { createFinance } from '/assets/finance.js';
import { records } from '/assets/storage.js';
import { parseUnits, formatUnits } from '/assets/sdk/index.js';

const $ = id => document.getElementById(id);
const encoder = new TextEncoder(), decoder = new TextDecoder();
let address = '', prepared = false, preparing = false, busy = false, planning = false, changingWallet = false, selected, chain, activePanel = 'payments';
let worker, viewWorker, sequence = 0, generation = 0, feedbackTimer, messageTimer, activity = [], lastRefreshHeight;
let balanceRequest=null,activityTimer,operationView=null,checkingOperation=false;
const dismissedOperations=new Set();
const transactionObservations=new Map();
const waiting = new Map();
export function message(text, error = false) {
  const box=$('message');clearTimeout(messageTimer);box.hidden=false;box.className=`message${error?' error':''}`;
  const copy=document.createElement('span');copy.textContent=text;const close=document.createElement('button');close.type='button';close.className='message-close';close.textContent='×';close.setAttribute('aria-label','Dismiss message');close.onclick=()=>{box.hidden=true;};box.replaceChildren(copy,close);
  for(const old of document.querySelectorAll('.form-error:not(#access-error)'))old.remove();
  const dialog=document.querySelector('dialog[open]');
  if(error&&dialog){const inline=document.createElement('p');inline.className='form-error';inline.setAttribute('role','alert');inline.textContent=text;dialog.querySelector('.dialog-head').after(inline);inline.scrollIntoView({block:'nearest'});}
  if(!/Authorizing|Building the proof|Minting/.test(text))messageTimer=setTimeout(()=>{box.hidden=true;},error?12000:5000);
}
const hex = bytes => Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
const amount = value => { const [a,b=''] = formatUnits(value).split('.'); return `${a}.${b.padEnd(6,'0')}`; };
function download(name, value) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value,null,2)+'\n'], {type:'application/json'}));
  const a = document.createElement('a'); a.href=url; a.download=name; a.click(); setTimeout(()=>URL.revokeObjectURL(url),1000);
}
function workerCall(type, value, bytes) {
  const publicData=['inspectNative','metadata','imageHash'].includes(type);
  let target=publicData?viewWorker:worker;
  if (!target) {
    target = new Worker('/assets/worker.js', {type:'module'});
    target.onmessage = ({data}) => { const promise=waiting.get(data.id); if(!promise)return;waiting.delete(data.id);data.ok ? promise.resolve({bytes:new Uint8Array(data.bytes),duration_ms:data.duration_ms}) : promise.reject(new Error(data.error)); };
    const current=target;
    target.onerror = () => { for(const [id,p]of waiting)if(p.worker===current){p.reject(new Error(publicData?'Asset data is reloading. Try again.':'Wallet worker stopped. Unlock the wallet again.'));waiting.delete(id);}if(publicData)viewWorker=undefined;else worker=undefined; };
    if(publicData)viewWorker=target;else worker=target;
  }
  const id = ++sequence;
  return new Promise((resolve,reject) => {waiting.set(id,{resolve,reject,worker:target});const data={id,type,value};if(bytes){data.bytes=bytes.buffer;target.postMessage(data,[bytes.buffer]);}else target.postMessage(data);});
}
async function api(path, body, binary=false,timeout=120000) {
  const immutableMedia=body===undefined&&/^\/api\/media\/(?:metadata|image)\/[0-9a-f]{64}$/.test(path);
  const response=await fetch(path,{method:body===undefined?'GET':'POST',headers:body===undefined?{}:{'Content-Type':binary?'application/octet-stream':'application/json'},body:body===undefined?undefined:binary?body:JSON.stringify(body),credentials:'omit',cache:immutableMedia?'default':'no-store',signal:AbortSignal.timeout(timeout)});
  if(!response.ok){const value=await response.json().catch(()=>({})),error=new Error(typeof value.error==='string'?value.error:`Request failed (${response.status})`);error.httpStatus=response.status;throw error;}
  return binary?new Uint8Array(await response.arrayBuffer()):response.json();
}
function applyAvailability() {
  const active = !!chain?.native.active_at_next_block && !!chain?.native.runtime_available;
  const enabled = active && !!address;
  $('activation-notice').hidden = active || !address;
  for (const button of document.querySelectorAll('[data-spend]')) button.disabled=busy||planning||changingWallet||!active||(!enabled&&!['launch-token','create-nft'].includes(button.id));
  for(const id of ['address-menu','wallet-settings'])$(id).disabled=busy||planning||changingWallet;
}
function preparation(text, ready=false) { $('preparation').textContent=text;$('preparation').className=`preparation${ready?' ready':''}`; }
async function prepare() {
  if(!address||preparing||prepared||busy||changingWallet||!chain?.native.active_at_next_block)return;
  const current=generation;preparing=true;preparation('Preparing authorization');
  try{await workerCall('prepare');if(current===generation){prepared=true;preparation('Ready to authorize',true);}}
  catch(e){if(current===generation){preparation('Preparation failed');message(e.message,true);}}
  finally{if(current===generation)preparing=false;}
}
async function status() {
  try {
    const next=await api('/api/status');const changed=lastRefreshHeight!==next.chain.height;chain=next;lastRefreshHeight=next.chain.height;
    $('node-status').textContent=`Block ${next.chain.height}`;$('node-dot').className='online';
    $('network-label').textContent=next.chain.testnet?'TESTNET':'MAINNET';
    finance.initialize();
    applyAvailability();
    if(address&&changed&&!busy){void refreshBalance().catch(()=>{});void finance.refresh(false).catch(()=>{});void refreshActivity().catch(()=>{});}
    else if(!address&&changed)void finance.refresh(false).catch(()=>{});
  }catch{$('node-status').textContent='Node unavailable';$('node-dot').className='';}
}
async function refreshBalance(fresh=false) {
  if(!address)return false;
  const owner=address,current=generation,previous=balanceRequest?.owner===owner&&balanceRequest.generation===current?balanceRequest.promise:null;
  if(previous&&!fresh)return previous;
  const task=(async()=>{
    if(previous)await previous.catch(()=>{});
    if(address!==owner||generation!==current)return false;
    const result=await api(`/api/balance/${encodeURIComponent(owner)}`);
    if(address!==owner||generation!==current)return false;
    $('balance').textContent=amount(result.balance_micronoid);finance.setNoid(result.balance_micronoid);return true;
  })();
  balanceRequest={owner,generation:current,promise:task};
  try{return await task;}finally{if(balanceRequest?.promise===task)balanceRequest=null;}
}

const unsettled=record=>!['Confirmed','Expired','Rejected','Not found'].includes(record.status)||record.balances_synced===false;
const canClearPending=record=>!!record&&unsettled(record)&&Date.now()-record.created>=15000&&transactionObservations.get(record.txid)?.can_clear_pending===true;
function operationStage(record){
  if(record.status==='Confirmed')return record.balances_synced===false?'updating':'complete';
  if(record.status==='Expired')return 'expired';
  if(record.status==='Rejected')return 'rejected';
  if(record.status==='Not found')return 'cleared';
  if(canClearPending(record))return 'missing';
  if(transactionObservations.get(record.txid)?.state==='unknown')return 'checking';
  return record.status==='Submitted'?'waiting':'checking';
}
function operationTitle(request){
  if(request?.action==='trade')return request.buy?'Token purchase':'Token sale';
  return ({create_token:'Token launch',transfer_token:'Token transfer',burn_token:'Burn & redeem',create_collection:'Collection creation',mint_nft:'NFT mint',mint_nfts:'NFT batch mint',transfer_nft:'NFT transfer',burn_nft:'NFT burn',list_nft:'NFT listing',buy_nft:'NFT purchase',cancel_listing:'Listing cancellation'})[request?.action]||'NOID transfer';
}
function setOperation(record,stage,detail='',force=false){
  if(record.address!==address)return;
  if(!force&&operationView&&operationView.txid!==record.txid&&operationView.created>record.created)return;
  operationView={...record,stage,detail};renderOperation();
}
function syncOperation(){
  if(busy&&operationView&&['authorizing','proving','submitting'].includes(operationView.stage)){renderOperation();return;}
  const next=activity.find(unsettled),current=activity.find(r=>r.txid===operationView?.txid);
  const record=next&&(!operationView||['complete','expired','failed','rejected','cleared'].includes(operationView.stage)&&next.created>operationView.created)?next:current||next;
  if(record){
    const stage=operationStage(record);
    setOperation(record,stage,operationView?.txid===record.txid&&stage==='updating'?operationView.detail:record.submission_error&&['rejected','checking'].includes(stage)?record.submission_error:'');
  }else renderOperation();
}
function renderOperation(){
  const pending=activity.filter(unsettled),draft=operationView&&['authorizing','proving','submitting'].includes(operationView.stage)&&!pending.some(r=>r.txid===operationView.txid),count=pending.length+(draft?1:0);
  for(const badge of document.querySelectorAll('[data-transaction-pending]')){badge.hidden=!address||!count;badge.textContent=count>1?`${count} Pending`:'Pending';}
  const box=$('transaction-status'),view=operationView;
  if(!view||view.address!==address||dismissedOperations.has(view.id)){box.hidden=true;$('transaction-reopen').hidden=!address||!count;return;}
  $('transaction-reopen').hidden=true;
  box.hidden=false;box.dataset.stage=view.stage;$('transaction-title').textContent=operationTitle(view.review?.request);
  const labels={authorizing:'Authorizing on your device',proving:'Building proof',submitting:'Submitting transaction',waiting:'Waiting for confirmation',checking:'Checking transaction status',missing:'Not found on the node',cleared:'Pending status cleared',rejected:'Submission rejected',updating:'Confirmed · Updating balances',complete:view.review?.request?.action==='trade'?(view.review.request.buy?'Purchase complete':'Sale complete'):'Transaction complete',expired:'Trade expired',failed:'Authorization stopped'};
  const descriptions={authorizing:'Your browser is authorizing this exact operation.',proving:'Preparing the proof for your authorized transaction.',submitting:'Sending the authorized transaction to your node.',waiting:'Submitted to your node. Waiting for inclusion in a block. You can prepare another operation using free funds.',checking:'Checking whether your transaction was accepted. Tracking continues automatically.',missing:'The synced node finds no confirmation or mempool entry. The submission may have been rejected or dropped.',cleared:'The record stays in Activity. Clearing Pending does not cancel a transaction. A later confirmation will still update its status.',rejected:'The node rejected this submission. Review a fresh operation to try again.',updating:`Included in block #${view.height}. Refreshing your NOID and asset balances.`,complete:`Confirmed in block #${view.height}. Your balances are up to date.`,expired:'This trade was not included before its deadline.',failed:'No completed transaction was submitted. You can review the operation again.'};
  const phase=labels[view.stage]||view.stage;if($('transaction-phase').textContent!==phase)$('transaction-phase').textContent=phase;
  const detail=view.detail||(view.stage==='checking'&&transactionObservations.get(view.txid)?.state==='unknown'?'The node is syncing. Confirmation cannot be checked yet. Tracking continues automatically.':descriptions[view.stage])||'';if($('transaction-detail').textContent!==detail)$('transaction-detail').textContent=detail;
  const terminal=['complete','expired','failed','rejected','cleared','missing'].includes(view.stage),step=['authorizing','proving','submitting','failed'].includes(view.stage)?0:['waiting','checking','expired','missing','rejected','cleared'].includes(view.stage)?1:2;
  for(const [index,item]of [...box.querySelectorAll('[data-transaction-step]')].entries()){
    item.classList.toggle('done',index<step||view.stage==='complete');item.classList.toggle('active',index===step&&!terminal);
  }
  $('transaction-id').textContent=view.txid?`${view.txid.slice(0,10)}…${view.txid.slice(-8)}`:'';$('transaction-id').title=view.txid||'';
  const stored=activity.find(record=>record.txid===view.txid);
  $('transaction-actions').hidden=!canClearPending(stored);
  $('transaction-clear').hidden=!canClearPending(stored);$('transaction-clear').disabled=checkingOperation||busy;
  $('transaction-clear').textContent=checkingOperation?'Checking…':'Clear pending';
  $('transaction-clear-note').hidden=!canClearPending(stored);
}
function showOperation(){
  const record=activity.find(unsettled);
  if(record&&(!operationView||!['authorizing','proving','submitting','waiting','checking','missing','updating'].includes(operationView.stage)))setOperation(record,operationStage(record),'',true);
  if(operationView){dismissedOperations.delete(operationView.id);renderOperation();}
}
function scheduleActivity(delay){
  clearTimeout(activityTimer);if(!address)return;
  activityTimer=setTimeout(()=>void refreshActivity().catch(()=>{}),delay??(activity.some(unsettled)?2000:15000));
}

async function observeTransaction(record,owner=address,current=generation){
  const observation=await api(`/api/transaction/${record.txid}/status`,undefined,false,20000);
  if(address!==owner||generation!==current)return;
  if(observation.txid!==record.txid||!['confirmed','pending','not_found','unknown'].includes(observation.state))throw new Error('Invalid transaction status response');
  transactionObservations.set(record.txid,observation);
  return records.update('transactions',record.id,latest=>{
    if(!latest||latest.address!==owner)return latest;
    if(observation.state==='confirmed'&&observation.transaction?.height!==undefined){
      if(latest.status==='Confirmed'&&latest.height===observation.transaction.height)return latest;
      return {...latest,status:'Confirmed',height:observation.transaction.height,balances_synced:false};
    }
    if(latest.status==='Confirmed')return latest;
    if(observation.state==='pending'&&latest.status!=='Submitted')return {...latest,status:'Submitted'};
    if(observation.state==='not_found'&&latest.action==='trade'&&Number(observation.checked_height)>Number(latest.review.valid_until)&&latest.status!=='Expired'){
      const updated={...latest,status:'Expired'};delete updated.balances_synced;return updated;
    }
    return latest;
  });
}
async function clearPending(record=operationView){
  if(!record||checkingOperation||busy||!address)return;
  const owner=address,current=generation;checkingOperation=true;renderOperation();
  try{
    let latest=await observeTransaction(record,owner,current);if(!latest||owner!==address||current!==generation)return;
    if(!canClearPending(latest))message(latest.status==='Confirmed'?'Already confirmed. Updating your balances.':transactionObservations.get(latest.txid)?.state==='pending'?'Still in the node mempool. Waiting for confirmation.':'The node cannot confirm that Pending can be cleared. Try checking again.',true);
    else{
      latest=await records.update('transactions',latest.id,value=>value&&!['Confirmed','Expired'].includes(value.status)?{...value,status:'Not found',pending_cleared_at:Date.now()}:value);
      if(owner!==address||current!==generation)return;
      if(latest.status==='Not found'){await finance.pendingCleared(latest);message('Pending cleared. The transaction record is kept in Activity.');}
    }
    activity=activity.map(value=>value.id===latest.id?latest:value);dismissedOperations.delete(latest.id);renderActivity();setOperation(latest,operationStage(latest),latest.status==='Rejected'?latest.submission_error||'':'',true);
  }catch(error){if(owner===address&&current===generation)message(`Could not check the transaction. Pending was kept. ${error.message}`,true);}
  finally{if(owner===address&&current===generation){checkingOperation=false;renderOperation();scheduleActivity(0);}}
}
function changePanel(name) {
  if(!address&&['payments','activity-panel'].includes(name)){showAccess();return;}
  showWorkspace();
  window.scrollTo({top:0,behavior:'instant'});
  activePanel=name;for(const panel of document.querySelectorAll('.panel'))panel.hidden=panel.id!==name;
  for(const tab of document.querySelectorAll('[data-panel]'))tab.classList.toggle('active',tab.dataset.panel===name);
  finance.panelChanged(name);if(name==='activity-panel')renderActivity();
  if(name==='payments')void finance.refreshDiscovery().catch(()=>{});
}
function showWorkspace(){ $('entry').hidden=true;$('entry-discovery').hidden=true;$('wallet').hidden=false;document.querySelector('.wallet-top').hidden=!address;if(!address)preparation('Connect your wallet to trade'); }
function showAccess(focusWallet=true){ for(const d of document.querySelectorAll('dialog[open]'))d.close();$('entry').hidden=false;$('entry-discovery').hidden=false;$('wallet').hidden=true;syncRoute(null);void finance.refreshDiscovery().catch(()=>{});if(focusWallet)document.querySelector('.access').scrollIntoView({block:'center',behavior:'smooth'});else window.scrollTo({top:0,behavior:'smooth'}); }
const desktopNavigation=matchMedia('(min-width:761px)');
function syncRoute(route){
  document.body.dataset.route=route||'wallet';$('mobile-wallet').classList.toggle('active',!route);$('mobile-wallet').setAttribute('aria-current',route?'false':'page');
  for(const b of document.querySelectorAll('button[data-route]')){const active=b.dataset.route===route;b.classList.toggle('active',active);b.setAttribute('aria-current',active?'page':'false');}
  const listings=desktopNavigation.matches&&!route&&activePanel==='nfts'&&$('nfts').dataset.view==='listings';
  for(const b of document.querySelectorAll('[data-panel], [data-wallet-view]')){const active=!route&&(b.dataset.walletView==='listings'?listings:b.dataset.panel===activePanel&&!(listings&&b.dataset.panel==='nfts'));b.classList.toggle('active',active);b.setAttribute('aria-current',active?'page':'false');}
}
desktopNavigation.addEventListener('change',()=>syncRoute(document.body.dataset.route==='wallet'?null:document.body.dataset.route));
document.addEventListener('wallet-route',event=>{syncRoute(event.detail);renderOperation();});
document.addEventListener('wallet-operation-view',renderOperation);
document.addEventListener('click',event=>{if(event.target.closest('[data-transaction-pending]'))showOperation();});
document.addEventListener('wallet-connect',()=>showAccess());
function fields(parent, pairs) {const dl=document.createElement('dl');for(const [name,value]of pairs){const dt=document.createElement('dt'),dd=document.createElement('dd');dt.textContent=name;dd.textContent=value;dl.append(dt,dd);}parent.append(dl);}
async function openReview(operation,review,options={}) {
  selected={operation:structuredClone(operation),review,options};
  $('review-details').replaceChildren();
  const request=review.request;
  const names={create_token:'Launch token',transfer_token:'Send tokens',burn_token:'Burn & redeem',trade:'Trade',create_collection:'Create collection',mint_nft:'Mint NFT',mint_nfts:'Mint NFTs',transfer_nft:'Send NFT',burn_nft:'Burn NFT',list_nft:'List NFT',buy_nft:'Buy NFT',cancel_listing:'Remove listing'};
  const title=request?names[request.action]:'Send NOID';
  fields($('review-details'),[['Action',title],['Authorizing address',review.authority],['Network fee',`${amount(review.fee_micronoid)} NOID`]]);
  if(request){
    const mediaIds=[...new Set([request.metadata_hash,...(request.metadata_hashes||[]),...(options.media||[])].filter(Boolean))];
    const media=await Promise.all(mediaIds.map(id=>records.get('media',id)));
    const first=media.find(Boolean)?.manifest;if(first)fields($('review-details'),[['Name',first.name],...(first.symbol?[['Ticker',first.symbol]]:[])]);
    if(request.whole_tokens)fields($('review-details'),[['Fixed supply',`${BigInt(request.whole_tokens).toLocaleString('en-US')} tokens`]]);
    if(request.initial_buy_units&&request.initial_buy_units!=='0')fields($('review-details'),[['Initial purchase',`${formatUnits(request.initial_buy_units)} tokens`]]);
    if(request.action==='mint_nft'||request.action==='mint_nfts')fields($('review-details'),[['NFTs to mint',String(mediaIds.length)],['Artwork',media.filter(Boolean).map(m=>m.manifest.name).join(', ')||mediaIds.join(', ')]]);
    if(request.recipient)fields($('review-details'),[['Recipient',request.recipient]]);
    if(request.units)fields($('review-details'),[['Tokens',formatUnits(request.units)]]);
    if(request.gross_input)fields($('review-details'),[['Spend up to',`${formatUnits(request.gross_input)} ${request.buy?'NOID':'tokens'}`]]);
    if(review.minimum_output)fields($('review-details'),[['Minimum receive',`${formatUnits(review.minimum_output)} ${request.buy?'tokens':'NOID'}`]]);
    if(review.quoted_output)fields($('review-details'),[['Current estimate',formatUnits(review.quoted_output)]]);
    if(review.valid_until)fields($('review-details'),[['Valid through block',String(review.valid_until)]]);
    if(review.fee_limit_micronoid)fields($('review-details'),[['Maximum network fee',`${amount(review.fee_limit_micronoid)} NOID`]]);
    if(review.reserve_deposit_micronoid)fields($('review-details'),[['Holder backing deposit',`${amount(review.reserve_deposit_micronoid)} NOID`]]);
    if(review.initial_buy_cost_micronoid&&review.initial_buy_cost_micronoid!=='0')fields($('review-details'),[['Creator purchase',`${amount(review.initial_buy_cost_micronoid)} NOID`]]);
    if(review.noid_debit_micronoid!=='0')fields($('review-details'),[[request.action==='trade'?'Estimated NOID debit':'Total NOID debit',`${amount(review.noid_debit_micronoid)} NOID`]]);
    if(review.noid_credit_micronoid!=='0')fields($('review-details'),[[request.action==='trade'?'Estimated NOID credit':'NOID credit',`${amount(review.noid_credit_micronoid)} NOID`]]);
    if(request.ask_micronoid||request.maximum_ask_micronoid)fields($('review-details'),[['NFT price',`${amount(request.ask_micronoid||request.maximum_ask_micronoid)} NOID`]]);
    if(request.expiry_height)fields($('review-details'),[['Listing expiry',String(request.expiry_height)]]);
    for(const key of ['asset_id','collection_id','serial'])if(request[key])fields($('review-details'),[[key.replaceAll('_',' '),request[key]]]);
  }else for(const [index,output]of review.outputs.entries())fields($('review-details'),[[`Output ${index+1}`,`${amount(output.amount_micronoid)} NOID`],['Recipient',output.address]]);
  $('review-note').textContent=options.note||'Your browser checks these terms and verifies the completed proof.';
  if(!options.auto){$('review-dialog').showModal();void prepare();}
  else return authorize();
}
async function authorize() {
  const plan=selected;selected=undefined;if(!plan||busy||changingWallet)return;
  busy=true;applyAvailability();$('review-dialog').close();const owner=address;
  const current=generation;
  const checkOwner=()=>{if(generation!==current||address!==owner)throw new Error('Wallet session changed. Review the operation again.');};
  const start=performance.now(),tracking={id:`${owner}:${plan.review.txid}`,address:owner,txid:plan.review.txid,review:plan.review,created:Date.now()};let record,relayAttempted=false;
  setOperation(tracking,'authorizing');
  try {
    while(preparing)await new Promise(r=>setTimeout(r,50));
    checkOwner();
    if(!prepared)await workerCall('prepare');
    checkOwner();
    prepared=false;preparation('Authorizing');
    const request=await workerCall('begin',plan.operation);
    checkOwner();
    preparation('Building proof');setOperation(tracking,'proving');
    const response=await api('/api/prove',request.bytes,true);
    checkOwner();
    const final=await workerCall('finish',undefined,response);
    checkOwner();
    const intent_hex=hex(final.bytes);
    record={...tracking,kind:plan.review.kind,action:plan.review.request?.action||'payment',status:'Check status',intent_hex,media:plan.options.media||[],flow:plan.options.flow||null};
    await records.put('transactions',record);
    setOperation(record,'submitting');
    checkOwner();
    relayAttempted=true;
    const txid=plan.operation.native_plan ? await finance.submit(intent_hex) : (await api('/api/relay',{intent_hex})).txid;
    if(txid!==record.txid)throw new Error('Node returned a different transaction identity');
    const observed=await records.get('transactions',record.id);
    record.status=observed?.status==='Confirmed'?'Confirmed':'Submitted';
    if(record.status==='Confirmed'){record.height=observed.height;record.balances_synced=false;}
    record.admission_ms=performance.now()-start;await records.put('transactions',record);
    if(address===owner){activity=[record,...activity.filter(r=>r.id!==record.id)];renderActivity();setOperation(record,record.status==='Confirmed'?'updating':'waiting');preparation('Submitted');await finance.submitted(record);}
    document.dispatchEvent(new CustomEvent('wallet-submitted',{detail:{txid,action:record.action}}));
  }catch(e){
    if(generation!==current)return;
    let cancelTimer,cancelTimedOut=false;
    try{await Promise.race([workerCall('cancel'),new Promise(resolve=>{cancelTimer=setTimeout(()=>{cancelTimedOut=true;resolve();},10000);})]);}catch{}finally{clearTimeout(cancelTimer);}
    if(generation!==current)return;
    if(record&&relayAttempted){
      const rejected=['RpcError','StateChangedError'].includes(e.name)||[400,409,422].includes(e.httpStatus);
      const failure={submission_error:String(e.message).slice(0,512),submission_error_at:Date.now(),...(Number.isInteger(e.code)?{submission_error_code:e.code}:{}),...(e.httpStatus?{submission_http_status:e.httpStatus}:{})};
      record=await records.update('transactions',record.id,latest=>latest?.status==='Confirmed'?{...latest,...failure}:{...(latest||record),...failure,...(rejected?{status:'Rejected'}:{})});
      if(generation!==current||address!==owner)return;
      activity=[record,...activity.filter(r=>r.id!==record.id)];renderActivity();setOperation(record,operationStage(record),record.status==='Confirmed'?'':record.submission_error);
      if(record.status!=='Confirmed')message(rejected?`Submission rejected: ${record.submission_error}`:`Submission status is uncertain: ${record.submission_error}`,true);
    }
    else{setOperation(tracking,'failed',e.message);message(e.message,true);}
    if(cancelTimedOut){await lockWallet();walletAccess.locked();message('The authorization worker stopped responding. Your transaction record is saved. Unlock the wallet again.',true);}
  }finally{if(generation===current){busy=false;applyAvailability();preparation('Ready to authorize',true);renderOperation();scheduleActivity(0);}}
}
function filterKind(record) {return record.action==='payment'?'payment':record.action==='trade'?'trade':record.action.includes('nft')||record.action.includes('collection')||record.action.includes('listing')?'nft':'token';}
function activityRow(record) {
  const row=document.createElement('article');row.className='activity-row';
  const icon=document.createElement('span');icon.className=`activity-icon ${filterKind(record)}`;icon.textContent=record.status==='Confirmed'?'✓':'↗';
  const body=document.createElement('div'),title=document.createElement('strong'),sub=document.createElement('span');title.textContent=record.action.replaceAll('_',' ');sub.textContent=`${record.txid.slice(0,12)}…${record.txid.slice(-8)}`;body.append(title,sub);
  const side=document.createElement('div');side.className='activity-side';const state=document.createElement('span');state.className=record.status==='Confirmed'?'positive':'muted';state.textContent=record.status;side.append(state);
  if(record.status==='Confirmed'&&record.balances_synced===false){const syncing=document.createElement('span');syncing.className='muted';syncing.textContent='Updating balances';side.append(syncing);}
  if(unsettled(record)||['Rejected','Not found'].includes(record.status)){const progress=document.createElement('button');progress.className='text-button';progress.textContent=unsettled(record)?'Track status':'View status';progress.onclick=()=>{dismissedOperations.delete(record.id);setOperation(record,operationStage(record),record.status==='Rejected'?record.submission_error||'':'',true);};side.append(progress);}
  if(canClearPending(record)){const clear=document.createElement('button');clear.className='text-button';clear.textContent='Clear pending';clear.onclick=()=>clearPending(record);side.append(clear);}
  const save=document.createElement('button');save.className='text-button';save.textContent=record.receipt_hex?'Save receipt':'Save record';save.onclick=()=>download(`parano1d-${record.txid.slice(0,12)}.json`,record);side.append(save);
  if(record.flow_ready){const resume=document.createElement('button');resume.className='text-button';resume.textContent='Resume minting';resume.onclick=()=>finance.resumeFlow(record.flow).catch(e=>message(e.message,true));side.append(resume);}
  row.append(icon,body,side);return row;
}
function renderActivity() {
  for(const [id,limit]of [['recent-activity',4],['activity',100]]){
    const container=$(id),filter=$('activity-filter').value;const rows=activity.filter(r=>id!=='activity'||filter==='all'||filterKind(r)===filter).slice(0,limit);
    container.replaceChildren(...rows.map(activityRow));if(!rows.length){const p=document.createElement('p');p.className='muted empty-copy';p.textContent='Your transactions and verified receipts appear here.';container.append(p);}
  }
}
let checkingActivity=null;
async function refreshActivity() {
  if(!address||checkingActivity?.owner===address&&checkingActivity.generation===generation)return;
  const owner=address,current=generation,ticket={owner,generation:current};checkingActivity=ticket;
  const stillCurrent=()=>owner===address&&current===generation;
  try{
    const previouslyPending=new Set(activity.filter(r=>!['Confirmed','Expired'].includes(r.status)).map(r=>r.id));
    const all=await records.all('transactions');if(!stillCurrent())return;
    activity=all.filter(r=>r.address===owner).sort((a,b)=>b.created-a.created).slice(0,100);
    for(const record of activity)if(record.status==='Confirmed'&&previouslyPending.has(record.id)){record.balances_synced=false;await records.put('transactions',record);}
    if(!stillCurrent())return;syncOperation();
    const candidates=activity.filter(r=>!['Confirmed','Expired'].includes(r.status)&&!(busy&&operationView?.txid===r.txid));
    const pending=[...candidates.filter(unsettled),...candidates.filter(r=>!unsettled(r))].slice(0,16);
    for(let i=0;i<pending.length;i+=4){
      const observations=await Promise.all(pending.slice(i,i+4).map(record=>observeTransaction(record,owner,current).catch(()=>undefined)));
      if(!stillCurrent())return;
      for(const record of observations)if(record)activity=activity.map(value=>value.id===record.id?record:value);
    }
    if(!stillCurrent())return;
    renderActivity();syncOperation();
    const syncing=activity.filter(r=>r.status==='Confirmed'&&r.balances_synced===false);
    if(syncing.length){
      try{
        const updated=await Promise.all([refreshBalance(true),finance.refreshBalances()]);
        if(!stillCurrent()||updated.some(value=>value!==true))return;
        for(const record of syncing){record.balances_synced=true;await records.put('transactions',record);}
        renderActivity();syncOperation();void finance.refresh(true).catch(()=>{});
      }catch{
        if(stillCurrent()&&operationView?.stage==='updating'){operationView.detail='Confirmed on chain. Balance updates are delayed. Retrying automatically.';renderOperation();}
      }
    }
    if(!stillCurrent())return;
    for(const record of activity.filter(r=>r.status==='Confirmed'&&r.flow))await finance.confirmed(record);
    const resumable=new Set();for(const record of activity){delete record.flow_ready;if(record.flow&&!resumable.has(record.flow)){resumable.add(record.flow);const flow=await records.get('flows',record.flow);record.flow_ready=flow?.status==='ready';}}
    for(const record of activity.filter(r=>r.status==='Confirmed'&&(!r.receipt_hex||r.media?.length)&&r.action!=='trade').slice(0,8)){
      try{const receipt=await api('/api/receipt',{txid:record.txid,intent_hex:record.intent_hex});record.receipt_hex=receipt.receipt_hex;await finance.publish(record);await records.put('transactions',record);}catch{}
    }
    if(stillCurrent()){renderActivity();syncOperation();}
  }finally{if(checkingActivity===ticket)checkingActivity=null;scheduleActivity();}
}
async function activateAddress(next) {
  const current=++generation;address=next;prepared=false;preparing=false;selected=undefined;activity=[];
  operationView=null;checkingOperation=false;dismissedOperations.clear();transactionObservations.clear();clearTimeout(activityTimer);renderOperation();
  preparation('Loading address');
  for(const dialog of document.querySelectorAll('dialog[open]'))dialog.close();
  $('wallet-address').textContent=address;$('balance').textContent='0.000000';
  $('header-wallet-label').textContent='My wallet';$('header-wallet').setAttribute('aria-label','My wallet');
  finance.changeOwner();renderActivity();showWorkspace();window.scrollTo({top:0,behavior:'instant'});applyAvailability();
  const results=await Promise.allSettled([status(),refreshBalance(),finance.connect(),refreshActivity()]);
  if(generation!==current)return;
  const failed=results.find(result=>result.status==='rejected');if(failed)message(failed.reason.message,true);
  if(chain?.native.active_at_next_block&&chain?.native.runtime_available)preparation('Ready to authorize',true);else preparation('Ready to receive');
}
async function lockWallet() {
  generation++;address='';prepared=false;preparing=false;busy=false;planning=false;changingWallet=false;selected=undefined;activity=[];
  operationView=null;checkingOperation=false;dismissedOperations.clear();transactionObservations.clear();clearTimeout(activityTimer);renderOperation();
  const previous=worker;worker=undefined;
  if(previous){previous.terminate();for(const [id,promise]of waiting)if(promise.worker===previous){promise.reject(new Error('Wallet locked'));waiting.delete(id);}}
  finance.disconnect();$('balance').textContent='0.000000';$('wallet-address').textContent='';renderActivity();
  for(const dialog of document.querySelectorAll('dialog[open]'))dialog.close();
  $('header-wallet-label').textContent='Open wallet ↗';$('header-wallet').setAttribute('aria-label','Open wallet');showAccess();applyAvailability();
}
const finance=createFinance({getAddress:()=>address,getChain:()=>chain,getPanel:()=>activePanel,isBusy:()=>busy||planning||changingWallet,setPlanning:value=>{planning=value;applyAvailability();},workerCall,api,message,openReview,changePanel,format:amount,records,download,refreshBalance});
const walletAccess=createWallet({workerCall,activate:activateAddress,lock:lockWallet,setWorking:value=>{changingWallet=value;applyAvailability();},isBusy:()=>busy||planning,getAddress:()=>address,getChain:()=>chain,api,message});
for(const button of document.querySelectorAll('[data-panel]'))button.onclick=()=>{if(!address){showAccess();return;}changePanel(button.dataset.panel);if(button.dataset.panel==='tokens'||button.dataset.panel==='nfts')finance.showHoldings(button.dataset.panel);};
for(const button of document.querySelectorAll('[data-wallet-view="listings"]'))button.onclick=()=>{if(!address){showAccess();return;}changePanel('nfts');finance.showListings();};
for(const button of document.querySelectorAll('button[data-route]'))button.onclick=()=>finance.browse(button.dataset.route).catch(e=>message(e.message,true));
$('header-wallet').onclick=()=>address?changePanel('payments'):showAccess();
$('mobile-wallet').onclick=()=>address?changePanel('payments'):showAccess();
document.querySelector('.brand').onclick=event=>{if(!event.metaKey&&!event.ctrlKey&&!event.shiftKey&&!event.altKey){event.preventDefault();address?changePanel('payments'):showAccess(false);}};
for(const button of document.querySelectorAll('[data-close]'))button.onclick=()=>$(button.dataset.close).close();
let popupViewportFrame;
function updatePopupViewport(){
  cancelAnimationFrame(popupViewportFrame);
  popupViewportFrame=requestAnimationFrame(()=>{
    const viewport=window.visualViewport;if(!viewport||viewport.scale!==1)return;
    for(const dialog of document.querySelectorAll('dialog[open]')){
      dialog.style.setProperty('--popup-height',`${viewport.height}px`);
      dialog.style.setProperty('--popup-top',`${viewport.offsetTop}px`);
      dialog.style.setProperty('--popup-bottom',`${Math.max(0,innerHeight-viewport.height-viewport.offsetTop)}px`);
      const input=document.activeElement;
      if(viewport.height<innerHeight-100&&input?.matches('input,textarea')&&dialog.contains(input))input.scrollIntoView({block:'nearest'});
    }
  });
}
window.visualViewport?.addEventListener('resize',updatePopupViewport);
window.visualViewport?.addEventListener('scroll',updatePopupViewport);
const popupScrollObserver=new MutationObserver(entries=>{for(const {target,oldValue}of entries)if(target.open&&oldValue===null){target.querySelector('.dialog-body')?.scrollTo(0,0);updatePopupViewport();}});
for(const dialog of document.querySelectorAll('dialog'))popupScrollObserver.observe(dialog,{attributes:true,attributeFilter:['open'],attributeOldValue:true});
const actionFooterObserver=new MutationObserver(()=>{if(!$('action-content').querySelector('.nft-detail-profile')){$('action-footer').hidden=true;$('action-footer').replaceChildren();}});
actionFooterObserver.observe($('action-content'),{childList:true});
$('authorize').onclick=()=>void authorize();$('activity-filter').onchange=renderActivity;
$('transaction-close').onclick=()=>{if(operationView)dismissedOperations.add(operationView.id);renderOperation();};
$('transaction-activity').onclick=()=>changePanel('activity-panel');
$('view-activity').onclick=()=>changePanel('activity-panel');
$('lock').onclick=async()=>{if(busy||planning||changingWallet){message('Wait for the current operation to finish.',true);return;}await lockWallet();walletAccess.locked();message('Wallet locked.');};
$('refresh').onclick=async()=>{try{await status();await refreshBalance();await finance.refresh(true);await refreshActivity();}catch(e){message(e.message,true);}};
$('transaction-clear').onclick=()=>clearPending();
$('copy-address').onclick=async event=>{const box=event.currentTarget.getBoundingClientRect();let failed=false;try{await navigator.clipboard.writeText(address);}catch{failed=true;}const label=$('copy-feedback');clearTimeout(feedbackTimer);label.hidden=false;label.textContent=failed?'Copy failed':'Copied';label.classList.toggle('error',failed);label.style.left=`${Math.max(8,Math.min(event.clientX||box.right,innerWidth-100))}px`;label.style.top=`${Math.max(8,(event.clientY||box.top)-42)}px`;feedbackTimer=setTimeout(()=>{label.hidden=true;},1500);};
$('receive').onclick=()=>openReceive({address,name:$('address-name').textContent,network:$('network-label').textContent});
$('send-form').onsubmit=async event=>{
  event.preventDefault();if(planning||busy||changingWallet)return;const current=generation,owner=address;try{
    const asset=$('send-asset').value;if(asset!=='NOID'){await finance.reviewTransfer(asset,$('recipient').value.trim(),parseUnits($('amount').value),parseUnits($('max-fee').value));return;}
    planning=true;applyAvailability();
    const request={owner:address,to:$('recipient').value.trim(),amount_micronoid:parseUnits($('amount').value),max_fee_micronoid:parseUnits($('max-fee').value)};
    const result=await api('/api/plan/payment',request);const review=JSON.parse(decoder.decode((await workerCall('review',result.operation)).bytes));
    if(current!==generation||address!==owner)return;
    if(review.authority!==address||review.kind!=='payment'||BigInt(review.fee_micronoid)>BigInt(request.max_fee_micronoid)||review.outputs[0]?.address!==request.to||review.outputs[0]?.amount_micronoid!==request.amount_micronoid||review.outputs.slice(1).some(o=>o.address!==address)||review.outputs.length>2)throw new Error('Payment recipient, amount or change was changed');
    if(!$('send-dialog').open)return;
    $('send-dialog').close();await openReview(result.operation,review);
  }catch(e){if(current===generation)message(e.message,true);}finally{if(current===generation){planning=false;applyAvailability();}}
};
document.addEventListener('visibilitychange',()=>{if(!document.hidden&&address)scheduleActivity(0);});
await status();setInterval(()=>void status(),10000);scheduleActivity();
