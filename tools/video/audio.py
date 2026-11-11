"""紹介動画の BGM（82 秒・48 kHz ステレオ）を合成して audio.wav に書く。numpy と scipy が必要。"""
import numpy as np, wave
sr=48000; D=82.0; n=int(sr*D); t=np.arange(n)/sr
rng=np.random.default_rng(3)
def env(t0,a,d,peak=1.0):
    e=np.zeros(n); i0=int(t0*sr); ia=int(a*sr); idd=int(d*sr)
    seg=np.concatenate([np.linspace(0,1,max(ia,1)),np.exp(-np.linspace(0,5,idd))])*peak
    seg=seg[:max(0,n-i0)]; e[i0:i0+len(seg)]=seg; return e
def lp(x,fc):
    a=np.exp(-2*np.pi*fc/sr); y=np.empty_like(x); z=0.0
    from scipy.signal import lfilter
    return lfilter([1-a],[1,-a],x)
def note(m): return 440*2**((m-69)/12)
L=np.zeros(n); R=np.zeros(n)
# chord pads: (start, end, midi notes)
chords=[(0,7,[38,50,57,62,65]),(7,17,[38,53,57,60,65]),(17,28,[34,53,58,62,65]),(28,39,[33,52,57,60,64]),
        (39,49,[41,53,57,60,67]),(49,61,[36,52,55,60,67]),(61,76,[41,53,60,65,69,72]),(76,82,[38,50,57,62,66,69])]
for s,e,ns in chords:
    g=np.clip((t-s+0.6)/1.6,0,1)*np.clip((e+1.0-t)/1.6,0,1); g=g*g*(3-2*g)
    for k,m in enumerate(ns):
        f=note(m)
        for det,pan in ((-0.004,0.25),(0.004,0.75)):
            ph=2*np.pi*f*(1+det)*t+rng.uniform(0,6.28)
            w=np.sin(ph)+0.35*np.sin(2*ph)+0.18*np.sin(3*ph)
            w*=g*(0.5+0.5*np.sin(2*np.pi*(0.07+0.013*k)*t+k))*0.05*(1.6 if k==0 else 1)
            L+=w*(1-pan); R+=w*pan
# arpeggio plucks 96 bpm 8ths from 7s to 76s (ducked in e-stop 33.6-39)
bpm=96; step=60/bpm/2
arps={7:[62,65,69,72],17:[62,65,70,74],28:[60,64,69,72],39:[60,65,69,72],49:[60,64,67,72],61:[65,69,72,77]}
keys=sorted(arps)
tt=7.0;i=0
while tt<75.5:
    k=[x for x in keys if x<=tt][-1]; m=arps[k][(i*3)%4]+(12 if i%8==5 else 0)
    amp=0.07*(0.5+0.5*min(1,(tt-7)/6))
    if 33.6<tt<39: amp*=0.0
    if tt>=61: amp*=1.25
    i0=int(tt*sr); ln=int(0.5*sr); ln=min(ln,n-i0); x=np.arange(ln)/sr
    w=(np.sin(2*np.pi*note(m)*x)+0.5*np.sin(4*np.pi*note(m)*x)*np.exp(-x*14))*np.exp(-x*9)*amp
    pan=0.3+0.4*((i%4)/3); L[i0:i0+ln]+=w*(1-pan); R[i0:i0+ln]+=w*pan
    # echo
    j0=i0+int(step*1.5*sr)
    if j0+ln<n: L[j0:j0+ln]+=w*0.35*pan; R[j0:j0+ln]+=w*0.35*(1-pan)
    tt+=step; i+=1
# soft kick on beats from 17s
tt=17.0
while tt<75.5:
    if not (33.6<tt<39.5):
        i0=int(tt*sr); ln=int(0.35*sr); x=np.arange(ln)/sr
        w=np.sin(2*np.pi*(45*x+ (110-45)/30*(1-np.exp(-x*30))))*np.exp(-x*11)*0.22
        L[i0:i0+ln]+=w; R[i0:i0+ln]+=w
    tt+=60/bpm
# scene hits + risers
for s in [0.3,7,17,28,39,49,61,76]:
    i0=int(s*sr); ln=int(2.5*sr); ln=min(ln,n-i0); x=np.arange(ln)/sr
    w=np.sin(2*np.pi*(38*x+25*(1-np.exp(-x*8))/8))*np.exp(-x*1.8)*0.32
    L[i0:i0+ln]+=w; R[i0:i0+ln]+=w
    if s>1:
        r0=int((s-1.6)*sr); ln=int(1.6*sr); x=np.arange(ln)/ln
        nz=lp(rng.standard_normal(ln),2500)*x**3*0.25
        L[r0:r0+ln]+=nz; R[r0:r0+ln]+=nz[::-1]*0+nz*0.9
# alarm: degraded blip at 31.64, e-stop beeps from 33.64
def beep(t0,f,d,a):
    i0=int(t0*sr); ln=int(d*sr); x=np.arange(ln)/sr
    w=np.sign(np.sin(2*np.pi*f*x))*0.4+np.sin(2*np.pi*f*x)*0.6
    w=lp(w,3000)*np.minimum(1,x*200)*np.minimum(1,(d-x)*200)*a
    L[i0:i0+ln]+=w; R[i0:i0+ln]+=w
beep(31.64,660,0.12,0.10); beep(31.82,660,0.12,0.10)
tb=33.64
while tb<37.6: beep(tb,880,0.16,0.12); beep(tb+0.22,660,0.16,0.10); tb+=0.66
# master
mix=np.stack([L,R],1)
fade=np.clip(t/1.0,0,1)*np.clip((D-t)/2.2,0,1)
mix*=fade[:,None]
mix=np.tanh(mix*1.5)/np.max(np.abs(np.tanh(mix*1.5)))*0.89
w=wave.open('audio.wav','wb'); w.setnchannels(2); w.setsampwidth(2); w.setframerate(sr); w.writeframes((mix*32767).astype('<i2').tobytes()); w.close()
print('ok',mix.shape)
