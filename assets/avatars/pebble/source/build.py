"""Build Pebble in a fresh Blender process."""
import sys
from pathlib import Path
import math
import random

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT.parent/'source'))
from character import Character
c=Character('pebble',ROOT)
for args in [('Stone','998FA4',.83,.10),('Eyes','161719',.12),('EyeRim','7D7882',.5),('White','FFFFFF',.2),('Mouth','51212B',.65),('Pink','F58C9C',.6),('Leaves','596D2F',.65,.22),('Vein','43551F',.65),('FleckLight','B9ACB6',.85),('FleckDark','797183',.85)]: c.material(*args)
body=c.sphere('Rounded stone',(0,.06,1.02),(.86,.56,.90),'Stone',segments=96,rings=72)
# The stone is full at its base, with a slightly narrow crown and soft facets.
for v in body.data.vertices:
    x,y,z=v.co; t=(z-1.02)/.90
    v.co.x*=(1-.11*t+.018*math.sin(z*9+y*4))*max(.01,1-t*t)**(-.10)
    if z<.40: v.co.z=.22+(z-.12)*.64
c.blush(body,'Stone',[(-.49,1.02),(.49,1.02)],(.20,.12))
for side in (-1,1):
    c.sphere('Foot',(side*.43,.04,.26),(.205,.25,.16),'Stone','Root')
    c.sphere('Arm',(side*.73,-.34,.67),(.16,.17,.17),'Stone')
c.face(.345,1.23,-.505,1.07,-.539,.132)
c.tube('Sprout stem',[(0,.06,1.87),(-.01,.055,2.05),(.035,.055,2.17)],.026,'Vein')
c.leaf('Left leaf',[(0,.05,2.05),(-.22,.035,2.07),(-.36,.04,2.23),(-.39,.05,2.38)],.137,'Leaves','Ear.L',.035)
c.leaf('Right leaf',[(.015,.055,2.10),(.18,.045,2.15),(.27,.04,2.29),(.26,.05,2.40)],.127,'Leaves','Ear.R',.035)
# Small mineral flecks follow the stone surface on all sides.
rng=random.Random(732)
for i in range(320):
    a=rng.uniform(0,math.tau); t=rng.uniform(-.70,.9)
    z=1.02+.90*t; r=math.sqrt(1-t*t)
    x=.86*r*math.cos(a)*(1-.11*t+.018*math.sin(z*9+.56*r*math.sin(a)*4))*max(.01,1-t*t)**(-.10)
    y=.06+.561*r*math.sin(a)
    if y<-.25 and ((abs(abs(x)-.345)<.16 and 1.07<z<1.39) or (abs(x)<.15 and .95<z<1.09)): continue
    s=rng.uniform(.003,.010)
    c.sphere('Mineral fleck',(x,y,z),(s,s*.08,s*.6),'FleckLight' if i%3 else 'FleckDark',segments=8,rings=6)
c.finish(.85,2.40,1.21)
