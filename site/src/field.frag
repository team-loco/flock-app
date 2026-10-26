precision highp float;
uniform vec2 res;uniform float cell,time,mode;uniform vec3 c1,c2,c3,c4,hi;
float h(vec2 p){p=fract(p*vec2(127.1,311.7));p+=dot(p,p+34.23);return fract(p.x*p.y);}
float vn(vec2 p){vec2 i=floor(p),f=fract(p),u=f*f*(3.-2.*f);return mix(mix(h(i),h(i+vec2(1,0)),u.x),mix(h(i+vec2(0,1)),h(i+vec2(1,1)),u.x),u.y);}
float fbm(vec2 p){float s=0.,a=.5;mat2 r=mat2(.8,-.6,.6,.8);for(int i=0;i<5;i++){s+=a*vn(p);p=r*p*2.02+17.3;a*=.5;}return s;}
float b2(vec2 a){a=floor(a);return fract(dot(a,vec2(.5,a.y*.75)));}
float b4(vec2 a){return b2(.5*a)*.25+b2(a);}
float b8(vec2 a){return b4(.5*a)*.25+b2(a);}
float mk(vec2 px,vec2 sz,float t){
  float side=smoothstep(.12,.48,abs(px.x/sz.x-.5)), m;
  if(mode<.5){float d=mix(260.,720.,side)*(.85+.3*vn(vec2(px.x/170.+t*.03,2.7)));m=1.-smoothstep(0.,d,px.y);}
  else{float d=mix(120.,sz.y*1.4,side)*(.85+.3*vn(vec2(px.x/170.+t*.03,6.1)));m=(1.-smoothstep(0.,d,px.y))*mix(.35,1.,side);}
  return m*(.5+.5*m);
}
float puff(vec2 q){
  if(q.y<0.)return 0.;
  float k=step(1.,q.x)*step(q.x,13.)*step(q.y,2.);
  k=max(k,step(length((q-vec2(4.,2.4))*vec2(1.,1.1)),2.6));
  k=max(k,step(length(q-vec2(7.6,3.6)),3.4));
  k=max(k,step(length((q-vec2(11.,2.3))*vec2(1.,1.1)),2.4));
  return k;
}
void main(){
  vec2 c=gl_FragCoord.xy, px=c*cell, sz=res*cell;
  float x=px.x/sz.x, up=px.y, t=time;
  vec2 p=px/240.+vec2(t*.02,-t*.011);
  vec2 q=vec2(fbm(p+vec2(0.,t*.03)),fbm(p+vec2(5.2,1.3)-vec2(t*.025,0.)));
  vec2 r=vec2(fbm(p+2.1*q+vec2(1.7,9.2)+t*.018),fbm(p+2.1*q+vec2(8.3,2.8)-t*.014));
  float n=smoothstep(.3,.8,fbm(p+2.*r));
  float m=mk(px,sz,t);
  float v=n*m*(mode<.5?1.12:1.25);
  vec2 dp=c+vec2(-t*2.,t*.15), sl=vec2(30.,17.), sid=floor(dp/sl), lp=floor(dp-sid*sl);
  float sc=.7+.55*h(sid+4.4);
  vec2 jo=floor(vec2(h(sid+1.3)*(28.-15.*sc),h(sid+7.7)*(15.-8.5*sc)));
  vec2 lq=(lp-jo)/sc; if(h(sid+2.9)<.5)lq.x=14.-lq.x;
  vec2 ctr=(sid*sl+jo+vec2(7.,4.)*sc-vec2(-t*2.,t*.15))*cell;
  float mc=mk(ctr,sz,t);
  float ok=mode<.5?smoothstep(180.,240.,ctr.y)*(1.-smoothstep(420.,500.,ctr.y))*smoothstep(.3,.4,abs(ctr.x/sz.x-.5)):smoothstep(.12,.5,mc)*smoothstep(70.,110.,ctr.y);
  if(h(sid+9.1)<(mode<.5?.16:.08)*ok&&puff(lq)>.5){gl_FragColor=vec4(hi,1.);return;}
  float l=floor(v*4.+b8(c));
  if(l<1.){gl_FragColor=vec4(0.);return;}
  gl_FragColor=vec4(l<2.?c1:l<3.?c2:l<4.?c3:c4,1.);
}
