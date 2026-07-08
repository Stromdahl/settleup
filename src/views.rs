//! HTML rendering with maud. Everything works as plain form POST + redirect; htmx
//! (vendored and served from our own origin, with `hx-boost`) is a progressive
//! enhancement, so the app is fully functional even if the script never loads.
//!
//! Visual direction ("SettleUp Redesign — Dark · blue", Sensative design system): a
//! near-black slate canvas with blue actions, avatar chips, and a glowing blue-gradient
//! "Settle up" hero — the one thing that glows. Green/red carry balance sign only. Fonts
//! fall back to the system stack (the DS ships a custom "Texta Alt" face, but the app
//! deliberately has no static-asset pipeline, so we stay self-contained / CSP-friendly).

use crate::money::format_amount;
use maud::{DOCTYPE, Markup, PreEscaped, html};
use std::collections::HashMap;

const STYLES: &str = r#"
:root{
  color-scheme:dark;
  --bg:#0b0c11; --bg-soft:#111219;
  --surface:#15171f; --surface-2:#0f1016; --surface-3:#1b1e29;
  --border:#262a36; --border-strong:#333a4a;
  --fg:#e9ebf2; --muted:#9ea4b3; --soft:#6f7687;
  --primary:#3f6fe5; --primary-2:#6b8afd; --primary-ink:#ffffff; --settle-amt:#a9c2ff;
  --note-bg:#141824; --note-border:#2b3450; --note-fg:#c3cdec; --note-muted:#8b93a8;
  --ok:#5CC58A; --ok-border:#1F8A5B; --ok-bg:#0f1a16; --alarm:#E07A6F;
  --r:16px; --r-lg:22px; --pill:999px;
  --font:"Texta Alt",ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
}
*{box-sizing:border-box;}
html,body{margin:0;}
body{font-family:var(--font);font-weight:400;font-size:16px;line-height:1.45;
  color:var(--fg);background:var(--bg);-webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility;}
::selection{background:rgba(63,111,229,.3);}
strong,b{font-weight:800;}
a{color:inherit;text-decoration:none;}

.wrap{max-width:600px;margin:0 auto;padding:34px 18px 46px;position:relative;min-height:100vh;}
.wrap.has-fab{padding-bottom:130px;}

/* ---- type ---- */
.eyebrow{font-size:12px;font-weight:800;letter-spacing:.12em;text-transform:uppercase;color:var(--primary-2);margin:0;}
.eyebrow.soft{color:var(--soft);}
h1,.title{font-size:34px;font-weight:800;letter-spacing:-.02em;line-height:1.05;margin:0;}
.gtitle{font-size:28px;font-weight:800;letter-spacing:-.02em;line-height:1.05;margin:3px 0 0;}
.sub{font-size:15px;color:var(--muted);margin:5px 0 0;}
.lead{font-size:17px;color:var(--muted);line-height:1.45;margin:10px 0 0;}
.section{font-size:12px;font-weight:800;letter-spacing:.1em;text-transform:uppercase;color:var(--soft);margin:26px 4px 10px;}
.section.spread{display:flex;align-items:baseline;justify-content:space-between;}
.section .total{font-weight:800;color:var(--soft);text-transform:none;letter-spacing:0;font-size:13px;}
.muted{color:var(--muted);} .soft{color:var(--soft);}
.name{flex:1;font-weight:800;font-size:16px;min-width:0;}

/* ---- cards & lists ---- */
.card{background:var(--surface);border:1px solid var(--border);border-radius:var(--r-lg);padding:20px;}
.list{background:var(--surface);border:1px solid var(--border);border-radius:var(--r);padding:2px 14px;}
.list .item{display:flex;align-items:center;gap:10px;padding:11px 0;border-bottom:1px solid var(--border);}
.list .item:last-child{border-bottom:0;}
.tile{background:var(--surface);border:1px solid var(--border);border-radius:var(--r);padding:14px 16px;}
.stackcol{display:flex;flex-direction:column;gap:10px;}

/* ---- labels & inputs ---- */
.field-label{display:block;font-size:12px;font-weight:800;letter-spacing:.06em;text-transform:uppercase;color:var(--soft);margin:16px 0 7px;}
form .field-label:first-of-type,.card>.field-label:first-child{margin-top:0;}
input[type=text],input[type=number],input[type=password],select{
  width:100%;box-sizing:border-box;padding:13px 14px;border:1px solid var(--border-strong);
  border-radius:12px;background:var(--surface-2);color:var(--fg);font-family:inherit;font-weight:800;font-size:17px;}
input::placeholder{color:var(--soft);font-weight:800;}
input:focus,select:focus{outline:none;border-color:var(--primary);box-shadow:0 0 0 3px rgba(63,111,229,.3);}
select{appearance:none;-webkit-appearance:none;padding-right:40px;background-repeat:no-repeat;background-position:right 14px center;
  background-image:url("data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='18'%20height='18'%20fill='none'%20stroke='%236f7687'%20stroke-width='2'%20stroke-linecap='round'%20stroke-linejoin='round'%3E%3Cpath%20d='m6%209%206%206%206-6'/%3E%3C/svg%3E");}

/* ---- buttons ---- */
.btn{display:inline-flex;align-items:center;justify-content:center;gap:10px;border:0;cursor:pointer;
  font-family:inherit;font-weight:800;font-size:18px;padding:16px 18px;border-radius:14px;
  background:var(--surface-3);color:var(--fg);text-decoration:none;line-height:1;}
.btn svg{width:20px;height:20px;stroke-width:2.2;}
.btn.primary{background:var(--primary);color:var(--primary-ink);}
.btn.primary svg{stroke-width:2.4;}
.btn.block{width:100%;}
.btn.ghost{background:var(--surface-2);border:1px solid var(--border-strong);color:var(--fg);}
.btn.sm{font-size:15px;padding:11px 16px;border-radius:12px;}
.btn:focus-visible{outline:none;box-shadow:0 0 0 3px rgba(63,111,229,.5);}

/* ---- avatars ---- */
.avatar{width:32px;height:32px;border-radius:999px;display:inline-flex;align-items:center;
  justify-content:center;font-weight:800;font-size:14px;flex:none;}
.avatar.sm{width:28px;height:28px;font-size:13px;}
.avatar.lg{width:34px;height:34px;font-size:15px;}
.stack{display:flex;}
.stack .avatar{border:2px solid var(--surface);}
.stack .avatar + .avatar{margin-left:-10px;}
.mini-badge{font-size:10px;font-weight:800;text-transform:uppercase;letter-spacing:.04em;
  color:var(--primary-2);border:1px solid var(--border-strong);padding:1px 6px;border-radius:999px;margin-left:6px;}

/* ---- group header ---- */
.ghead{display:flex;align-items:flex-start;justify-content:space-between;gap:12px;padding:0 4px;}
.iconbtn{width:40px;height:40px;border-radius:999px;background:var(--surface);border:1px solid var(--border);
  display:inline-flex;align-items:center;justify-content:center;color:var(--muted);flex:none;}
.iconbtn svg{width:22px;height:22px;stroke-width:1.7;}
.badge{display:inline-block;font-size:12px;font-weight:800;letter-spacing:.02em;background:var(--surface-3);
  color:var(--muted);padding:5px 12px;border-radius:999px;}

/* ---- settle-up hero ---- */
.settle{margin-top:18px;border-radius:22px;padding:20px;color:#fff;
  background:linear-gradient(155deg,#1c2a63 0%,#2f5fd6 100%);box-shadow:0 20px 46px -18px rgba(63,111,229,.55);}
.settle-top{display:flex;align-items:center;justify-content:space-between;}
.settle-top .eyebrow{color:var(--settle-amt);}
.settle-count{font-size:12px;font-weight:800;color:rgba(255,255,255,.7);}
.settle-head{font-size:22px;font-weight:800;letter-spacing:-.01em;line-height:1.2;margin-top:6px;}
.xfers{display:flex;flex-direction:column;gap:10px;margin-top:16px;}
.xfer{display:flex;align-items:center;gap:12px;background:rgba(255,255,255,.1);border-radius:14px;padding:12px;}
.xfer .pair{display:flex;align-items:center;gap:6px;color:rgba(255,255,255,.55);}
.xfer .pair svg{width:16px;height:16px;stroke-width:2.2;}
.xfer .who{flex:1;min-width:0;}
.xfer .who .names{font-size:14px;color:rgba(255,255,255,.75);}
.xfer .who .amt{font-size:19px;font-weight:800;color:var(--settle-amt);line-height:1.1;}
.xfer .who .amt .cur{font-size:12px;color:rgba(255,255,255,.6);}
.mark{display:inline-flex;align-items:center;gap:6px;background:var(--primary);color:var(--primary-ink);
  border:0;border-radius:999px;font-family:inherit;font-weight:800;font-size:14px;padding:10px 14px;
  white-space:nowrap;cursor:pointer;}
.mark svg{width:15px;height:15px;stroke-width:2.6;}

/* ---- settled / empty states ---- */
.state{margin-top:18px;border-radius:22px;padding:28px 20px;text-align:center;background:var(--ok-bg);border:1px solid var(--ok-border);}
.state .disc{width:64px;height:64px;border-radius:999px;display:inline-flex;align-items:center;justify-content:center;
  background:var(--ok);color:#ffffff;box-shadow:0 10px 26px -8px rgba(92,197,138,.6);}
.state .disc svg{width:34px;height:34px;stroke-width:2.8;}
.state .state-title{font-size:24px;font-weight:800;letter-spacing:-.01em;margin-top:14px;}
.state .state-sub{font-size:15px;color:var(--muted);margin-top:6px;line-height:1.45;}
.empty{margin-top:14px;border:1.5px dashed var(--border-strong);border-radius:18px;padding:26px 20px;text-align:center;}
.empty .disc{width:48px;height:48px;border-radius:999px;background:var(--surface-3);color:var(--soft);
  display:inline-flex;align-items:center;justify-content:center;}
.empty .disc svg{width:24px;height:24px;}
.empty .empty-title{font-size:17px;font-weight:800;margin-top:12px;}
.empty .empty-sub{font-size:14px;color:var(--muted);margin-top:5px;line-height:1.45;}

/* ---- balances ---- */
.bal .amt{text-align:right;}
.bal .amt .k{font-size:11px;color:var(--soft);font-weight:800;text-transform:uppercase;letter-spacing:.04em;}
.bal .amt .v{font-size:16px;font-weight:800;}
.v.pos{color:var(--ok);} .v.neg{color:var(--alarm);} .v.zero{color:var(--soft);}

/* ---- expense & payment rows ---- */
.xrow-top{display:flex;align-items:baseline;justify-content:space-between;gap:10px;}
.xrow-top .desc{font-size:16px;font-weight:800;}
.xrow-top .amt{font-size:16px;font-weight:800;}
.xrow-meta{display:flex;align-items:center;justify-content:space-between;margin-top:5px;gap:10px;}
.xrow-meta .who{font-size:13px;color:var(--muted);min-width:0;}
.xrow-meta .rt{display:flex;align-items:center;gap:12px;flex:none;}
.xrow-meta .time{font-size:12px;color:var(--soft);}
.del{font-family:inherit;font-size:13px;font-weight:800;color:var(--alarm);background:none;border:0;cursor:pointer;padding:0;}
.edit{font-family:inherit;font-size:13px;font-weight:800;color:var(--primary-2);cursor:pointer;}
.pay{display:flex;align-items:center;justify-content:space-between;gap:10px;}
.pay .pname{font-size:15px;font-weight:800;}
.pay .ptime{font-size:12px;color:var(--soft);margin-top:3px;}
.pay .pamt{font-size:16px;font-weight:800;color:var(--ok);flex:none;}

/* ---- invite ---- */
.invite{text-align:center;}
.invite .eyebrow{margin-bottom:2px;}
.invite .invite-title{font-size:20px;font-weight:800;letter-spacing:-.01em;margin-top:6px;line-height:1.2;}
.qr{display:inline-block;background:#fff;padding:14px;border-radius:16px;margin-top:16px;}
.qr svg{display:block;width:168px;height:168px;}
.linkrow{display:flex;align-items:center;gap:10px;margin-top:16px;background:var(--surface-2);border:1px solid var(--border);
  border-radius:12px;padding:12px 14px;text-align:left;}
.linkrow .lk{flex:1;min-width:0;}
.linkrow .lk .k{font-size:11px;font-weight:800;letter-spacing:.06em;text-transform:uppercase;color:var(--soft);}
.linkrow .lk .u{font-size:14px;font-weight:800;display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
.copy{display:inline-flex;align-items:center;gap:6px;background:var(--primary);color:var(--primary-ink);border:0;border-radius:10px;
  font-family:inherit;font-size:14px;font-weight:800;padding:9px 14px;white-space:nowrap;cursor:pointer;flex:none;}
.copy svg{width:15px;height:15px;stroke-width:2;}

/* ---- segmented toggle (pure CSS, no JS) ---- */
.seg{display:flex;gap:4px;background:var(--surface-2);border-radius:12px;padding:4px;}
.seg input{position:absolute;opacity:0;pointer-events:none;}
.seg label{flex:1;text-align:center;padding:10px;border-radius:9px;font-size:15px;font-weight:800;color:var(--muted);cursor:pointer;}
.seg input:checked + label{background:var(--primary);color:#fff;}
.seg label:focus-within{box-shadow:0 0 0 3px rgba(63,111,229,.45);}

/* ---- share list (custom checkbox) ---- */
.share-row{display:flex;align-items:center;gap:12px;padding:11px 0;border-bottom:1px solid var(--border);}
.share-row:last-child{border-bottom:0;}
.chk{appearance:none;-webkit-appearance:none;width:24px;height:24px;border-radius:8px;border:2px solid var(--border-strong);
  background:var(--surface-2);position:relative;cursor:pointer;flex:none;margin:0;}
.chk:checked{background:var(--primary);border-color:var(--primary);}
.chk:checked::after{content:"";position:absolute;left:7px;top:3px;width:6px;height:11px;
  border:solid #ffffff;border-width:0 3px 3px 0;transform:rotate(45deg);}
.chk:focus-visible{outline:none;box-shadow:0 0 0 3px rgba(63,111,229,.45);}
.share-row input[type=text]{width:96px;flex:none;padding:9px 10px;font-size:15px;text-align:right;border-radius:10px;}
.share-row input[type=text][readonly]{background:var(--surface-2);color:var(--muted);cursor:default;}
.share-hint{font-size:13px;color:var(--soft);margin:-2px 0 10px;}

/* ---- note / owner card ---- */
.note{border-radius:16px;padding:14px 16px;background:var(--note-bg);border:1px solid var(--note-border);}
.note-head{display:flex;align-items:center;gap:8px;font-size:14px;font-weight:800;color:var(--note-fg);}
.note-head svg{width:17px;height:17px;color:var(--primary);stroke-width:1.8;flex:none;}
.note-body{font-size:13px;color:var(--note-muted);margin-top:6px;line-height:1.45;}
.note input[type=password]{margin-top:12px;}
.row-actions{display:flex;flex-wrap:wrap;gap:10px;margin-top:12px;}
.inlineform{display:inline;}

/* ---- landing ---- */
.brand{display:flex;flex-direction:column;align-items:flex-start;gap:20px;}
.logo{color:var(--primary-2);display:inline-flex;}
.brand .title{font-size:34px;}
.foot{text-align:center;font-size:13px;color:var(--soft);margin-top:14px;line-height:1.5;}

/* ---- join social proof ---- */
.members-preview{display:flex;align-items:center;gap:10px;margin-top:22px;background:var(--surface);
  border:1px solid var(--border);border-radius:16px;padding:14px 16px;}
.members-preview .cnt{font-size:14px;color:var(--muted);line-height:1.35;}
.recover-link{margin-top:28px;text-align:center;}
.recover-link .q{font-size:14px;color:var(--muted);}
.recover-link a{display:inline-block;font-size:15px;font-weight:800;color:var(--primary-2);
  margin-top:4px;text-decoration:underline;text-underline-offset:3px;}

/* ---- sticky Add-expense action ---- */
.fabbar{position:fixed;left:0;right:0;bottom:0;padding:14px 16px calc(18px + env(safe-area-inset-bottom,0px));
  background:linear-gradient(0deg,var(--bg) 62%,rgba(11,31,34,0));z-index:20;}
.fabbar-inner{max-width:600px;margin:0 auto;}
.fabbar .btn{box-shadow:0 12px 30px -10px rgba(63,111,229,.42);}

/* ---- live-update "someone joined" banner ---- */
.joinbar{display:flex;align-items:center;gap:12px;margin-bottom:16px;padding:12px 14px;
  border-radius:14px;background:var(--surface-3);border:1px solid var(--border-strong);}
.joinbar .jb-text{flex:1;font-weight:800;font-size:15px;min-width:0;}
.joinbar .jb-refresh{font-weight:800;font-size:14px;color:var(--primary-2);white-space:nowrap;}
.joinbar .jb-x{background:none;border:0;color:var(--soft);font-size:22px;line-height:1;
  padding:0 2px;cursor:pointer;font-family:inherit;}

/* ---- add-expense screen (its own focused page) ---- */
.addhead{display:flex;align-items:center;justify-content:space-between;margin-bottom:18px;}
.addhead h1{font-size:28px;}
.closebtn{width:34px;height:34px;border-radius:999px;background:var(--surface-3);flex:none;
  display:inline-flex;align-items:center;justify-content:center;color:var(--muted);}
.closebtn svg{width:18px;height:18px;stroke-width:2.2;}
.total-card{background:var(--surface);border:1px solid var(--border);border-radius:18px;padding:18px;text-align:center;}
.total-card .k{display:block;font-size:12px;font-weight:800;letter-spacing:.08em;text-transform:uppercase;color:var(--soft);}
.total-row{display:flex;align-items:baseline;justify-content:center;gap:8px;margin-top:6px;}
input.total-in{width:auto;min-width:2ch;field-sizing:content;border:0;padding:0;background:transparent;
  text-align:center;font-size:44px;letter-spacing:-.02em;line-height:1;}
input.total-in:focus{outline:none;box-shadow:none;border:0;}
.total-row .cur{font-size:18px;font-weight:800;color:var(--soft);}
"#;

/// Copy-to-clipboard for the join link. Progressive enhancement only: without JS the
/// link is still shown and selectable, so nothing depends on this.
const INLINE_JS: &str = r#"document.addEventListener('click',function(e){var b=e.target.closest('[data-copy]');if(!b)return;e.preventDefault();var t=b.getAttribute('data-copy');if(navigator.clipboard){navigator.clipboard.writeText(t).then(function(){var s=b.querySelector('.copy-label')||b;var o=s.textContent;s.textContent='Copied';setTimeout(function(){s.textContent=o;},1200);});}});
document.addEventListener('click',function(e){var d=e.target.closest('[data-dismiss]');if(!d)return;var n=d.closest(d.getAttribute('data-dismiss'));if(n)n.remove();});"#;

// Live preview of the equal ("balanced") split on the add/edit expense form. In equal
// mode the per-person amount fields are derived (the server splits `amount` across the
// ticked members and ignores the `amt_` fields), so we render them read-only and keep
// them in sync as the total, the checkboxes, or the split method change. This mirrors
// `settle::equal_shares` exactly — integer öre, leftover öre handed one at a time to the
// lowest member ids (the server sorts included ids before splitting) — so the preview
// matches what gets saved. Pure progressive enhancement: no-JS still submits correctly.
const FORM_JS: &str = r#"
(function(){
  function parseOre(s){
    s=(s||'').replace(/^\s+|\s+$/g,'').replace(',', '.');
    if(s==='')return null;
    var dot=s.indexOf('.'), ip, fp;
    if(dot<0){ip=s;fp='';}else{ip=s.slice(0,dot);fp=s.slice(dot+1);}
    if(ip==='')ip='0';
    if(!/^\d+$/.test(ip))return null;
    var frac=0;
    if(fp.length===1){if(!/^\d$/.test(fp))return null;frac=parseInt(fp,10)*10;}
    else if(fp.length>=2){var f2=fp.slice(0,2);if(!/^\d\d$/.test(f2))return null;frac=parseInt(f2,10);}
    return parseInt(ip,10)*100+frac;
  }
  function fmtOre(o){var sg=o<0?'-':'';o=Math.abs(o);var m=o%100;return sg+Math.floor(o/100)+'.'+(m<10?'0'+m:m);}
  function amtOf(row){return row.querySelector('input[name^="amt_"]');}
  function chkOf(row){return row.querySelector('input[name^="inc_"]');}
  function idOf(row){var c=chkOf(row);var m=c&&c.name.match(/^inc_(\d+)$/);return m?parseInt(m[1],10):null;}
  function sync(form){
    if(!form||!form.querySelector('input[name="method"]'))return;
    var r=form.querySelector('input[name="method"]:checked');
    var equal=!r||r.value==='equal';
    var rows=Array.prototype.slice.call(form.querySelectorAll('.share-row'));
    rows.forEach(function(row){var a=amtOf(row);if(a)a.readOnly=equal;});
    if(!equal)return;
    var totalEl=form.querySelector('input[name="amount"]');
    var total=parseOre(totalEl?totalEl.value:'');
    rows.forEach(function(row){var a=amtOf(row);if(a)a.value='';});
    var ticked=rows.filter(function(row){var c=chkOf(row);return c&&c.checked;});
    ticked.sort(function(a,b){return idOf(a)-idOf(b);});
    var n=ticked.length;
    if(total===null||n===0)return;
    var base=Math.floor(total/n), rem=total-base*n;
    ticked.forEach(function(row,i){var a=amtOf(row);if(a)a.value=fmtOre(base+(i<rem?1:0));});
  }
  function onEvt(e){
    var t=e.target;
    if(!t||!t.name)return;
    if(t.name==='amount'||t.name==='method'||/^inc_\d+$/.test(t.name))sync(t.form||t.closest('form'));
  }
  document.addEventListener('input',onEvt);
  document.addEventListener('change',onEvt);
  function init(){var m=document.querySelector('form input[name="method"]');if(m)sync(m.form);}
  document.addEventListener('DOMContentLoaded',init);
  document.addEventListener('htmx:load',init);
})();
"#;

// --- Inline icons (stroke uses currentColor; sized via width/height, tuned in CSS) ----

const P_ARROW: &str = r#"<path d="M5 12h14"/><path d="m13 5 7 7-7 7"/>"#;
const P_CHECK: &str = r#"<path d="m4 12 5 5L20 6"/>"#;
const P_PLUS: &str = r#"<path d="M12 5v14"/><path d="M5 12h14"/>"#;
const P_SHARE: &str = r#"<path d="M4 12v8a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-8"/><path d="M16 6l-4-4-4 4"/><path d="M12 2v14"/>"#;
const P_COPY: &str =
    r#"<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h10"/>"#;
const P_LOCK: &str =
    r#"<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>"#;
const P_CLOSE: &str = r#"<path d="M6 6 18 18"/><path d="M18 6 6 18"/>"#;
const LOGO: &str = r#"<svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v6"/><path d="M12 16v6"/><path d="M2 12h6"/><path d="M16 12h6"/><path d="m5 5 4.5 4.5"/><path d="m14.5 14.5 4.5 4.5"/><path d="m19 5-4.5 4.5"/><path d="m5 19 4.5-4.5"/><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/></svg>"#;

fn icon(paths: &str, size: u32) -> Markup {
    PreEscaped(format!(
        r#"<svg width="{size}" height="{size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{paths}</svg>"#
    ))
}

/// Deterministic `(background, foreground)` avatar colour for a member id, drawn from
/// the design-system palette. Mustard is reserved for actions, so it's not used here.
fn avatar_colors(id: i64) -> (&'static str, &'static str) {
    const PALETTE: &[(&str, &str)] = &[
        ("#3f6fe5", "#ffffff"), // blue (matches the primary action)
        ("#698FB2", "#ffffff"), // silver lake
        ("#B8818B", "#ffffff"), // lavender
        ("#5AA98F", "#ffffff"), // muted teal-green
        ("#8C7CC0", "#ffffff"), // periwinkle
        ("#C0876B", "#ffffff"), // warm clay
    ];
    PALETTE[(id.rem_euclid(PALETTE.len() as i64)) as usize]
}

fn initial(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default()
}

/// A round avatar chip. `extra` is any extra class (e.g. `"sm"` / `"lg"`).
fn avatar(name: &str, id: i64, extra: &str) -> Markup {
    let (bg, fg) = avatar_colors(id);
    let class = if extra.is_empty() {
        "avatar".to_string()
    } else {
        format!("avatar {extra}")
    };
    html! { span class=(class) style={ "background:" (bg) ";color:" (fg) } { (initial(name)) } }
}

/// Spell small counts ("Three payments square everyone.").
fn count_word(n: usize) -> String {
    const W: [&str; 10] = [
        "Zero", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
    ];
    W.get(n)
        .map(|s| s.to_string())
        .unwrap_or_else(|| n.to_string())
}

/// SQLite timestamps are `YYYY-MM-DD HH:MM:SS` (UTC). Render as e.g. `Jul 3 · 21:14`.
fn fmt_dt(s: &str) -> String {
    let b = s.as_bytes();
    if s.len() >= 16 && b.get(4) == Some(&b'-') {
        const MON: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let mi: usize = s[5..7].parse().unwrap_or(0);
        let mon = MON.get(mi.wrapping_sub(1)).copied().unwrap_or("");
        let day: u32 = s[8..10].parse().unwrap_or(0);
        let hm = &s[11..16];
        if !mon.is_empty() && day > 0 {
            return format!("{mon} {day} · {hm}");
        }
    }
    s.get(..16).unwrap_or(s).to_string()
}

/// Strip the scheme for a compact link display; the full URL stays in href/copy.
fn display_url(u: &str) -> &str {
    u.strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u)
}

fn layout(title: &str, wrap_extra: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="theme-color" content="#0b0c11";
                title { (title) }
                style { (PreEscaped(STYLES)) }
                script src="/assets/htmx-2.0.4.min.js" defer {}
                script { (PreEscaped(INLINE_JS)) (PreEscaped(FORM_JS)) }
            }
            body hx-boost="true" {
                main class={ "wrap " (wrap_extra) } { (body) }
            }
        }
    }
}

/// Landing page: create a new group.
pub fn landing() -> Markup {
    layout(
        "SettleUp",
        "",
        html! {
            div.brand {
                span.logo { (PreEscaped(LOGO)) }
                div {
                    div.title { "SettleUp" }
                    p.lead { "Split a bar tab or your monthly expenses. Everyone adds what they paid — then settle up in the fewest payments." }
                }
            }
            form method="post" action="/" style="margin-top:22px" {
                div.card {
                    label.field-label for="name" { "Group name" }
                    input type="text" name="name" id="name" placeholder="Friday drinks" required;
                    label.field-label for="your_name" { "Your name" }
                    input type="text" name="your_name" id="your_name" placeholder="Alex" required;
                    label.field-label for="currency" { "Currency" }
                    input type="text" name="currency" id="currency" value="SEK" maxlength="3";
                }
                button.btn.primary.block type="submit" style="margin-top:18px" {
                    "Create group" (icon(P_ARROW, 20))
                }
            }
            p.foot { "No accounts. No install." br; "Just share the link that comes next." }
        },
    )
}

/// Shown when a visitor lands on a group link but isn't a member yet.
pub fn claim(group: &crate::models::Group, members: &[MemberRow], total: i64) -> Markup {
    let owner = members
        .iter()
        .find(|m| m.is_owner)
        .map(|m| m.name.as_str())
        .unwrap_or("Someone");
    let count = members.len();
    let people = if count == 1 { "person" } else { "people" };
    layout(
        &format!("Join {}", group.name),
        "",
        html! {
            p.eyebrow { "You're invited to" }
            h1.title style="margin-top:6px" { (group.name) }
            p.lead { b { (owner) } " started this tab. Add your name to join — the running total updates as everyone chips in." }

            @if !members.is_empty() {
                div.members-preview {
                    div.stack {
                        @for m in members.iter().take(4) { (avatar(&m.name, m.id, "")) }
                    }
                    div.cnt {
                        b { (count) " " (people) } " in so far"
                        @if total > 0 { " · " (format_amount(total)) " " (group.currency) " on the tab" }
                    }
                }
            }

            form method="post" action={ "/g/" (group.id) "/join" } style="margin-top:22px" {
                label.field-label for="name" { "Your name" }
                input type="text" name="name" id="name" placeholder="Your name" required autofocus;
                button.btn.primary.block type="submit" style="margin-top:16px" { "Join " (group.name) }
            }

            @if group.has_recovery() {
                div.recover-link {
                    div.q { "Started this group before?" }
                    a href={ "/g/" (group.id) "/recover" } { "Owner? Recover access" }
                }
            }
        },
    )
}

/// Recovery page: enter the passphrase to re-claim owner access on a new device.
pub fn recover(group: &crate::models::Group, error: bool) -> Markup {
    layout(
        &format!("Recover {}", group.name),
        "",
        html! {
            p.eyebrow { "Owner access" }
            h1.title style="margin-top:6px" { "Recover “" (group.name) "”" }
            p.lead { "Enter the recovery passphrase set for this group to restore owner access on this device." }
            @if error {
                div.tile style="margin-top:14px;border-color:var(--alarm)" {
                    span style="color:var(--alarm);font-weight:800" { "That passphrase didn't match." }
                }
            }
            form method="post" action={ "/g/" (group.id) "/recover" } style="margin-top:18px" {
                label.field-label for="passphrase" { "Recovery passphrase" }
                input type="password" name="passphrase" id="passphrase" required autofocus;
                button.btn.primary.block type="submit" style="margin-top:16px" { "Recover access" }
            }
        },
    )
}

pub struct MemberRow {
    pub id: i64,
    pub name: String,
    pub is_owner: bool,
}
pub struct BalanceRow {
    pub id: i64,
    pub name: String,
    pub net: i64,
    pub is_owner: bool,
}
pub struct TransferRow {
    pub from_id: i64,
    pub from: String,
    pub to_id: i64,
    pub to: String,
    pub amount: i64,
}
pub struct ExpenseRow {
    pub id: i64,
    pub payer: String,
    pub amount: i64,
    pub description: String,
    pub participants: String,
    pub created_at: String,
    /// May the current viewer edit or delete this expense (payer or owner)?
    pub can_manage: bool,
}
pub struct SettlementRow {
    pub from: String,
    pub to: String,
    pub amount: i64,
    pub created_at: String,
}

pub struct GroupView<'a> {
    pub group: &'a crate::models::Group,
    pub me: &'a crate::models::Member,
    pub join_url: &'a str,
    pub members: Vec<MemberRow>,
    pub balances: Vec<BalanceRow>,
    pub transfers: Vec<TransferRow>,
    pub expenses: Vec<ExpenseRow>,
    pub settlements: Vec<SettlementRow>,
    /// Change token for the live-update poller (see `db::group_version`).
    pub version: i64,
}

/// The invite block: QR + copyable join link. Rendered as the hero for a brand-new
/// group, and lower down (still reachable via the header share icon) once it's active.
fn invite_card(join_url: &str, hero: bool) -> Markup {
    html! {
        section.card.invite id="invite" {
            p.eyebrow { "Invite the table" }
            @if hero { div.invite-title { "Scan to join in seconds" } }
            div.qr { (PreEscaped(qr_svg(join_url))) }
            div.linkrow {
                div.lk {
                    div.k { "Join link" }
                    a.u href=(join_url) { (display_url(join_url)) }
                }
                button.copy type="button" data-copy=(join_url) {
                    (icon(P_COPY, 15)) span.copy-label { "Copy" }
                }
            }
        }
    }
}

/// The main group page.
pub fn group_page(v: &GroupView) -> Markup {
    let g = v.group;
    let cur = &g.currency;
    let closed = g.is_closed();
    let empty = v.expenses.is_empty();
    let member_count = v.members.len();

    layout(
        &g.name,
        if closed { "" } else { "has-fab" },
        html! {
            // Live-update plumbing: a hidden 5s poller and a slot for the "someone
            // joined" notice. Both are progressive enhancement — inert without htmx.
            (poller(&g.id, v.version, member_count as i64, i64::from(closed), false))
            div id="ls-notice" {}

            // ---- Header ----
            div.ghead {
                div {
                    p.eyebrow.soft { "Group · " (cur) }
                    h1.gtitle { (g.name) }
                    p.sub {
                        "You're " b { (v.me.name) }
                        @if v.me.is_owner { " · owner" }
                        " · " (frag_count(v, false))
                    }
                    @if closed { p style="margin-top:8px" { span.badge { "Closed" } } }
                }
                a.iconbtn href="#invite" aria-label="Invite others" { (icon(P_SHARE, 22)) }
            }

            @if closed {
                div.tile style="margin-top:14px" {
                    span.muted { "This group is closed — no new expenses or payments." }
                }
            }

            // ---- Hero zone (live) ----
            (frag_hero(v, false))

            // ---- Balances, once there's something to balance (live) ----
            (frag_balances(v, false))

            // Add-expense lives on its own focused screen (GET /g/{id}/add), reached via
            // the sticky button below; the group page itself is entirely read-only, which
            // is exactly why the live poller can swap any region here without clobbering
            // in-progress input (there is none). See decisions #10 and #11.

            // ---- Expense log + payments, or the member list when empty (live) ----
            (frag_ledger(v, false))

            // ---- Owner controls ----
            @if v.me.is_owner {
                div.section { "Owner" }
                @if !g.has_recovery() {
                    form method="post" action={ "/g/" (g.id) "/recovery" } .note {
                        div.note-head { (icon(P_LOCK, 17)) span { "This tab disappears in 3 days unless you keep it" } }
                        div.note-body { "Set a recovery passphrase to keep it forever and restore access on another device." }
                        input type="password" name="passphrase" id="passphrase" placeholder="Recovery passphrase" required;
                        div.row-actions { button.btn.primary.sm type="submit" { "Keep this group" } }
                    }
                } @else {
                    div.note {
                        div.note-head { (icon(P_LOCK, 17)) span { "Recovery passphrase set · this group is kept" } }
                        @if !closed {
                            div.note-body { "Settle & close when you're done for the month — you can reopen anytime." }
                        }
                    }
                }
                div.row-actions {
                    @if !closed {
                        form.inlineform method="post" action={ "/g/" (g.id) "/close" } {
                            button.btn.ghost.sm type="submit" { "Settle & close" }
                        }
                    } @else {
                        form.inlineform method="post" action={ "/g/" (g.id) "/reopen" } {
                            button.btn.primary.sm type="submit" { "Reopen group" }
                        }
                    }
                }
            }

            // ---- Sticky primary action ----
            @if !closed {
                div.fabbar {
                    div.fabbar-inner {
                        a.btn.primary.block href={ "/g/" (g.id) "/add" } {
                            (icon(P_PLUS, 20))
                            @if empty { "Add first expense" } @else { "Add expense" }
                        }
                    }
                }
            }
        },
    )
}

/// The "New expense" screen — its own focused page (frame 04 of the redesign), reached
/// from the group's sticky Add-expense button. Posts to the same `/g/{id}/expenses`
/// endpoint as before and redirects back to the group on success.
/// Everything the shared expense form needs beyond the group and roster. Built once
/// for a new expense (defaults) and once for an edit (prefilled from the stored row).
struct ExpenseFormData {
    /// POST target for the form.
    action: String,
    /// Page heading / `<title>` prefix, e.g. "New expense" or "Edit expense".
    heading: &'static str,
    /// Submit-button label.
    submit: &'static str,
    description: String,
    /// Formatted total (`""` for a brand-new expense).
    total: String,
    payer_id: i64,
    /// Whether the "Exact amounts" method is pre-selected (equal otherwise).
    exact: bool,
    /// Per member id: `(included?, formatted-amount-or-empty)`.
    shares: HashMap<i64, (bool, String)>,
}

fn expense_form(group: &crate::models::Group, members: &[MemberRow], f: ExpenseFormData) -> Markup {
    let g = group;
    let cur = &g.currency;
    layout(
        &format!("{} · {}", f.heading, g.name),
        "",
        html! {
            div.addhead {
                h1 { (f.heading) }
                a.closebtn href={ "/g/" (g.id) } aria-label="Cancel" { (icon(P_CLOSE, 18)) }
            }
            form method="post" action=(f.action) {
                div.total-card {
                    label.k for="amount" { "Total" }
                    div.total-row {
                        input.total-in type="text" name="amount" id="amount" value=(f.total)
                            inputmode="decimal" placeholder="0.00" autofocus;
                        span.cur { (cur) }
                    }
                }

                label.field-label for="description" { "What for?" }
                input type="text" name="description" id="description" value=(f.description)
                    placeholder="Dinner, taxi, groceries…" required;

                label.field-label for="payer_id" { "Who paid?" }
                select name="payer_id" id="payer_id" {
                    @for m in members {
                        option value=(m.id) selected[m.id == f.payer_id] { (m.name) }
                    }
                }

                label.field-label { "Split" }
                div.seg {
                    input type="radio" name="method" id="m-equal" value="equal" checked[!f.exact];
                    label for="m-equal" { "Equally" }
                    input type="radio" name="method" id="m-exact" value="exact" checked[f.exact];
                    label for="m-exact" { "Exact amounts" }
                }

                label.field-label { "Who shares it?" }
                p.share-hint { "Tick who's in for an equal split, or type each person's amount for exact." }
                div.list {
                    @for m in members {
                        @let (inc, amt) = f.shares.get(&m.id).cloned().unwrap_or((false, String::new()));
                        div.share-row {
                            input.chk type="checkbox" name={ "inc_" (m.id) } value="1" checked[inc];
                            (avatar(&m.name, m.id, "sm"))
                            span.name { (m.name) }
                            input type="text" name={ "amt_" (m.id) } value=(amt) inputmode="decimal" placeholder="0.00";
                        }
                    }
                }

                button.btn.primary.block type="submit" style="margin-top:20px" { (f.submit) }
            }
        },
    )
}

pub fn add_expense_page(
    group: &crate::models::Group,
    me: &crate::models::Member,
    members: &[MemberRow],
) -> Markup {
    // New expense: everyone ticked, no amounts, equal split, payer = current member.
    let shares = members
        .iter()
        .map(|m| (m.id, (true, String::new())))
        .collect();
    expense_form(
        group,
        members,
        ExpenseFormData {
            action: format!("/g/{}/expenses", group.id),
            heading: "New expense",
            submit: "Add expense",
            description: String::new(),
            total: String::new(),
            payer_id: me.id,
            exact: false,
            shares,
        },
    )
}

/// Edit form for an existing expense, prefilled from the stored row and its shares.
/// The method toggle defaults to "Exact amounts" with each stored share filled in —
/// always faithful, since the original equal/exact choice isn't persisted. Re-splitting
/// equally (e.g. to fold in a newcomer) is one radio tap away.
pub fn edit_expense_page(
    group: &crate::models::Group,
    members: &[MemberRow],
    expense_id: i64,
    payer_id: i64,
    description: &str,
    total_ore: i64,
    current_shares: &[(i64, i64)],
) -> Markup {
    let included: HashMap<i64, i64> = current_shares.iter().copied().collect();
    let shares = members
        .iter()
        .map(|m| match included.get(&m.id) {
            Some(&amt) => (m.id, (true, format_amount(amt))),
            None => (m.id, (false, String::new())),
        })
        .collect();
    expense_form(
        group,
        members,
        ExpenseFormData {
            action: format!("/g/{}/expenses/{}/edit", group.id, expense_id),
            heading: "Edit expense",
            submit: "Save changes",
            description: description.to_string(),
            total: format_amount(total_ore),
            payer_id,
            exact: true,
            shares,
        },
    )
}

// --- Live-update fragments ------------------------------------------------------
//
// The read-only regions of the group page, each wrapped in a stable `id` so the live
// poll can swap them out-of-band without touching the add-expense form between them.
// Every fragment renders identically in the full page (`oob = false`) and in the poll
// response (`oob = true`, which adds `hx-swap-oob`). See decision #10 in DECISIONS.md.

/// The member-count text in the header — its own target so a join updates it live.
fn frag_count(v: &GroupView, oob: bool) -> Markup {
    let n = v.members.len();
    html! {
        span id="ls-count" hx-swap-oob=[oob.then_some("true")] {
            (n) " member" @if n != 1 { "s" }
        }
    }
}

/// The hero: invite + empty state before any expense, the "settle up" transfers once
/// there's debt, or the "all settled" state.
fn frag_hero(v: &GroupView, oob: bool) -> Markup {
    let g = v.group;
    let cur = &g.currency;
    let closed = g.is_closed();
    let empty = v.expenses.is_empty();
    html! {
        div id="ls-hero" hx-swap-oob=[oob.then_some("true")] {
            @if empty {
                (invite_card(v.join_url, true))
                div.empty {
                    span.disc { (icon(P_PLUS, 24)) }
                    div.empty-title { "No expenses yet" }
                    div.empty-sub { "Add the first thing someone paid for and balances appear automatically." }
                }
            } @else if !closed && !v.transfers.is_empty() {
                section.settle {
                    div.settle-top {
                        span.eyebrow { "Settle up" }
                        span.settle-count { (v.transfers.len()) " payment" @if v.transfers.len() != 1 { "s" } }
                    }
                    div.settle-head {
                        @if v.transfers.len() == 1 { "One payment squares everyone." }
                        @else { (count_word(v.transfers.len())) " payments square everyone." }
                    }
                    div.xfers {
                        @for t in &v.transfers {
                            div.xfer {
                                div.pair {
                                    (avatar(&t.from, t.from_id, "sm"))
                                    (icon(P_ARROW, 16))
                                    (avatar(&t.to, t.to_id, "sm"))
                                }
                                div.who {
                                    div.names { (t.from) " → " (t.to) }
                                    div.amt { (format_amount(t.amount)) " " span.cur { (cur) } }
                                }
                                form.inlineform method="post" action={ "/g/" (g.id) "/settlements" } {
                                    input type="hidden" name="from_id" value=(t.from_id);
                                    input type="hidden" name="to_id" value=(t.to_id);
                                    input type="hidden" name="amount_ore" value=(t.amount);
                                    button.mark type="submit" { (icon(P_CHECK, 15)) "Mark paid" }
                                }
                            }
                        }
                    }
                }
            } @else if !closed {
                section.state {
                    span.disc { (icon(P_CHECK, 34)) }
                    div.state-title { "All settled up" }
                    div.state-sub { "Nobody owes anybody. Add an expense to start the next round." }
                }
            }
        }
    }
}

/// The per-member balance list (empty until the first expense).
fn frag_balances(v: &GroupView, oob: bool) -> Markup {
    let empty = v.expenses.is_empty();
    html! {
        div id="ls-balances" hx-swap-oob=[oob.then_some("true")] {
            @if !empty {
                div.section { "Balances" }
                div.list {
                    @for b in &v.balances {
                        div.item.bal {
                            (avatar(&b.name, b.id, ""))
                            span.name {
                                (b.name)
                                @if b.is_owner { span.mini-badge { "owner" } }
                            }
                            div.amt {
                                @if b.net > 0 {
                                    div.k { "is owed" }
                                    div.v.pos { "+" (format_amount(b.net)) }
                                } @else if b.net < 0 {
                                    div.k { "owes" }
                                    div.v.neg { "−" (format_amount(-b.net)) }
                                } @else {
                                    div.v.zero { "settled" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The expense + settlement logs and the secondary invite once populated, or the member
/// list while the tab is empty.
fn frag_ledger(v: &GroupView, oob: bool) -> Markup {
    let g = v.group;
    let cur = &g.currency;
    let empty = v.expenses.is_empty();
    let expense_total: i64 = v.expenses.iter().map(|e| e.amount).sum();
    html! {
        div id="ls-ledger" hx-swap-oob=[oob.then_some("true")] {
            @if !empty {
                div.section.spread {
                    span { "Expenses" }
                    span.total { (format_amount(expense_total)) " " (cur) " total" }
                }
                div.stackcol {
                    @for e in &v.expenses {
                        div.tile {
                            div.xrow-top {
                                span.desc { (e.description) }
                                span.amt { (format_amount(e.amount)) }
                            }
                            div.xrow-meta {
                                span.who { (e.payer) " paid · " (e.participants) }
                                div.rt {
                                    span.time { (fmt_dt(&e.created_at)) }
                                    @if e.can_manage && !g.is_closed() {
                                        a.edit href={ "/g/" (g.id) "/expenses/" (e.id) "/edit" } { "Edit" }
                                    }
                                    @if e.can_manage {
                                        form.inlineform method="post" action={ "/g/" (g.id) "/expenses/" (e.id) "/delete" } {
                                            button.del type="submit" { "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                @if !v.settlements.is_empty() {
                    div.section { "Payments" }
                    div.stackcol {
                        @for s in &v.settlements {
                            div.tile.pay {
                                div {
                                    div.pname { (s.from) " paid " (s.to) }
                                    div.ptime { (fmt_dt(&s.created_at)) }
                                }
                                div.pamt { (format_amount(s.amount)) }
                            }
                        }
                    }
                }

                (invite_card(v.join_url, false))
            } @else {
                div.section { "Members" }
                div.list {
                    @for m in &v.members {
                        div.item {
                            (avatar(&m.name, m.id, ""))
                            span.name {
                                (m.name)
                                @if m.is_owner { span.mini-badge { "owner" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The hidden element that polls `/g/{id}/live` every 5s. It carries the state the client
/// last rendered — change token, member count, closed flag — so the server can answer
/// 204 / OOB / `HX-Refresh`. `hx-swap="none"` means the body is ignored; only its
/// out-of-band swaps apply. On a content response the server re-sends this element (OOB)
/// with a bumped token, so the next tick reflects what was just rendered.
fn poller(gid: &str, version: i64, member_count: i64, closed_flag: i64, oob: bool) -> Markup {
    let url = format!("/g/{gid}/live?v={version}&m={member_count}&c={closed_flag}");
    html! {
        div id="ls-poll" hx-swap-oob=[oob.then_some("true")]
            hx-get=(url) hx-trigger="every 5s" hx-swap="none" style="display:none" {}
    }
}

/// The dismissible "someone joined" banner, swapped into the persistent `#ls-notice`
/// slot. "Refresh" is a boosted navigation that rebuilds the page — and its form — with
/// the newcomer selectable; the × clears the banner (leaving the slot for the next join).
fn join_notice(gid: &str) -> Markup {
    html! {
        div id="ls-notice" hx-swap-oob="true" {
            div.joinbar {
                span.jb-text { "Someone just joined." }
                a.jb-refresh href={ "/g/" (gid) } { "Refresh to include them" }
                button.jb-x type="button" data-dismiss=".joinbar" aria-label="Dismiss" { "×" }
            }
        }
    }
}

/// The out-of-band response for the 5-second poll: the read-only fragments (never the
/// input form), a bumped poller token, and — when the roster grew — a join notice.
pub fn live_update(v: &GroupView, joined: bool) -> Markup {
    let closed_flag = i64::from(v.group.is_closed());
    html! {
        (frag_count(v, true))
        (frag_hero(v, true))
        (frag_balances(v, true))
        (frag_ledger(v, true))
        (poller(&v.group.id, v.version, v.members.len() as i64, closed_flag, true))
        @if joined { (join_notice(&v.group.id)) }
    }
}

/// Render a QR code for the given URL as an inline SVG string.
fn qr_svg(url: &str) -> String {
    use qrcode::QrCode;
    use qrcode::render::svg;
    match QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(180, 180)
            .quiet_zone(false)
            .build(),
        Err(_) => String::new(),
    }
}
