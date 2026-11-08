/* The app's fluid material, with a larger body for the website illustration. */
(async () => {
  'use strict';
  const image=document.querySelector('.app-orb'),stage=image?.parentElement;
  if(!image||!stage)return;
  const canvas=document.createElement('canvas');
  canvas.className='app-orb site-orb';canvas.setAttribute('aria-hidden','true');
  canvas.style.cssText='position:absolute;left:50%;top:50%;transform:translate(-50%,-50%)';
  const gl=canvas.getContext('webgl',{alpha:true,premultipliedAlpha:true,antialias:false});
  if(!gl)return;
  let program,buffer,locations,request=0,last=0,phase=0,inView=true;
  const reduced=matchMedia('(prefers-reduced-motion: reduce)');
  const compile=(kind,source)=>{
    const shader=gl.createShader(kind);gl.shaderSource(shader,source);gl.compileShader(shader);
    if(!gl.getShaderParameter(shader,gl.COMPILE_STATUS)){gl.deleteShader(shader);throw Error('Graphics unavailable');}
    return shader;
  };
  const response=await fetch('site-assets/orb.frag').catch(()=>null);
  if(!response?.ok)return;
  const source=(await response.text()).replace('float radius = .34','float radius = .512');
  function setup(){
    const vertex=compile(gl.VERTEX_SHADER,'attribute vec2 position;void main(){gl_Position=vec4(position,0.,1.);}');
    const fragment=compile(gl.FRAGMENT_SHADER,source);
    program=gl.createProgram();gl.attachShader(program,vertex);gl.attachShader(program,fragment);gl.linkProgram(program);
    gl.deleteShader(vertex);gl.deleteShader(fragment);
    if(!gl.getProgramParameter(program,gl.LINK_STATUS))throw Error('Graphics unavailable');
    gl.useProgram(program);buffer=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,buffer);
    gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,1,-1,-1,1,-1,1,1,-1,1,1]),gl.STATIC_DRAW);
    const position=gl.getAttribLocation(program,'position');gl.enableVertexAttribArray(position);gl.vertexAttribPointer(position,2,gl.FLOAT,false,0,0);
    locations=Object.fromEntries(['resolution','tint','phase','energy','listening','thinking','speaking','still'].map(name=>[name,gl.getUniformLocation(program,name)]));
  }
  function draw(now){
    request=0;if(document.hidden||!inView||gl.isContextLost())return;
    if(now-last>=32||reduced.matches){
      const dt=Math.min(Math.max((now-last)/1000,0),.06);last=now;if(!reduced.matches)phase+=dt*.18;
      const size=Math.max(1,Math.round(Math.min(stage.clientWidth,490)*Math.min(devicePixelRatio||1,1.5)));
      if(canvas.width!==size){canvas.width=size;canvas.height=size;gl.viewport(0,0,size,size);}
      gl.uniform2f(locations.resolution,size,size);gl.uniform3f(locations.tint,.525,.8,1);
      for(const [name,value]of Object.entries({phase,energy:.12,listening:0,thinking:0,speaking:0,still:reduced.matches?1:0}))gl.uniform1f(locations[name],value);
      gl.drawArrays(gl.TRIANGLES,0,6);
    }
    if(!reduced.matches)request=requestAnimationFrame(draw);
  }
  function resume(){cancelAnimationFrame(request);request=0;last=performance.now()-33;if(!document.hidden&&inView)request=requestAnimationFrame(draw);}
  canvas.addEventListener('webglcontextlost',event=>{event.preventDefault();cancelAnimationFrame(request);canvas.hidden=true;image.hidden=false;});
  canvas.addEventListener('webglcontextrestored',()=>{try{setup();image.hidden=true;canvas.hidden=false;resume();}catch{canvas.hidden=true;image.hidden=false;}});
  try{setup();stage.append(canvas);image.hidden=true;resume();}catch{return;}
  document.addEventListener('visibilitychange',resume);window.addEventListener('resize',resume);reduced.addEventListener('change',resume);
  if('IntersectionObserver'in window)new IntersectionObserver(entries=>{inView=entries[0].isIntersecting;resume();}).observe(stage);
})();
