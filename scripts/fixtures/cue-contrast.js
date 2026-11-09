'use strict';
const pause=()=>new Promise(r=>setTimeout(r,0));
const linear=v=>{v/=255;return v<=0.04045?v/12.92:((v+0.055)/1.055)**2.4};
const lum=p=>.2126*linear(p[0])+.7152*linear(p[1])+.0722*linear(p[2]);
const contrast=(a,b)=>(Math.max(a,b)+.05)/(Math.min(a,b)+.05);
const hues=[0,30,35,60,90,120,135,150,180,195,210,240,270,280,300,330];
// Integrated fields need 3:1 against their adjacent core, not a fixed badge colour.
const cues=['working','attention','finished','review'];
async function run(){
 const output=document.querySelector('pre'),w=document.querySelector('iframe').contentWindow;
 const result={passed:false,samples:0,minimumContrast:Infinity,brightestObserved:{shader:0,canvas:0,white:1},failures:[],target:3};
 try{
  const end=performance.now()+15000;while(!w.wasmBindings?.CueAcceptance&&performance.now()<end)await new Promise(r=>setTimeout(r,100));
  if(!w.wasmBindings?.CueAcceptance)throw Error('Build debug WASM before running the contrast checks');
  for(const size of [320,480]){
   const canvas=w.document.createElement('canvas');canvas.width=canvas.height=size;
   const renderer=new w.wasmBindings.CueAcceptance(canvas),ctx=canvas.getContext('2d',{willReadFrequently:true});
   const c=size/2,r=size*.17,b=Math.min(Math.max(r*.23,11),20,r*.28);
   const darkest=(x,y)=>{const p=ctx.getImageData(Math.round(x)-1,Math.round(y)-1,3,3).data;let best=p.slice(0,4);for(let i=4;i<p.length;i+=4){const q=p.slice(i,i+4);if(lum(q)<lum(best))best=q;}return best;};
   const light=(x,y)=>{const p=ctx.getImageData(Math.round(x)-1,Math.round(y)-1,3,3).data;let n=0;for(let i=0;i<p.length;i+=4)n=Math.max(n,lum(p.slice(i,i+4)));return n;};
   for(const hue of hues){
    for(let frame=0;frame<24;frame++)for(const reduced of [false,true])for(const background of ['shader','canvas','white']){
     const phase=frame*4;
     renderer.draw(hue,phase,'none',reduced,background);
     const edge=Math.floor(c-r),width=Math.ceil(r*2),image=ctx.getImageData(edge,edge,width,width).data;
     for(let y=edge;y<c+r;y+=3)for(let x=edge;x<c+r;x+=3){if((x-c)**2+(y-c)**2<r*r*.9){const i=((y-edge)*width+x-edge)*4;result.brightestObserved[background]=Math.max(result.brightestObserved[background],lum(image.slice(i,i+4)));}}
     for(const cue of cues){
      renderer.draw(hue,phase,cue,reduced,background);
      const t=reduced?0:phase/12;
      const radius=r*(background==='canvas'?1.35*(1+(reduced?0:.035+Math.sin(phase)*.005)):(reduced?1:1.075));
      const point=(x,y)=>[c+x*radius,c+y*radius];
      let points,back;
      if(cue==='working'){
       const y=.14*Math.sin(t*.3)-.22*Math.sin(t*.65);points=[point(0,y)];back=point(0,y>0?y-.55:y+.55);
      }else if(cue==='attention'){
       points=[point(0,0)];back=point(.85,0);
      }else if(cue==='finished'){
       points=[point(0,0)];back=point(.94,0);
      }else{points=[point(-.34,.32),point(0,.4),point(.34,.32)];back=point(0,-.6);}
      const backing=darkest(...back),dark=lum(backing);
      for(const sample of points){const ratio=contrast(light(...sample),dark);result.samples++;if(ratio<result.minimumContrast){result.minimumContrast=ratio;result.worstCase={size,hue,phase,reduced,background,cue};}if((ratio<3||backing[3]!==255)&&result.failures.length<12)result.failures.push({size,hue,phase,reduced,background,cue,ratio,backing:Array.from(backing)});}

     }
    }
    output.textContent=JSON.stringify({...result,current:{size,hue}},null,2);await pause();
   }
   renderer.free();
  }
  result.passed=result.minimumContrast>=3&&result.failures.length===0;
 }catch(error){result.failures.push(String(error));}
 window.cueContrastResults=result;output.textContent=JSON.stringify(result,null,2);document.title=result.passed?'PASS cue contrast':'FAIL cue contrast';
}
document.querySelector('button').onclick=run;
