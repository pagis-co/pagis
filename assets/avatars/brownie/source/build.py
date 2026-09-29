"""Build Brownie in a fresh Blender process."""
import sys
from pathlib import Path
import math
from mathutils import Vector

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT.parent/'source'))
from character import Character
c=Character('brownie',ROOT)
for args in [('Skin','FFE0AB',.58),('Face','FFE0AB',.58),('Hat','9F422E',.86,.16),('HatEdge','BD6746',.83,.14),('Hair','D5AA69',.78),('Tunic','A97A3F',.87,.18),('Hem','D9B579',.85,.4),('Leather','795027',.7,.18),('LeatherLight','A57639',.75),('Nose','DC795A',.57),('Eyes','24190E',.13),('EyeRim','9A713D',.48),('White','FFFFFF',.2),('Mouth','804329',.7),('Pink','F3988A',.6),('Leaves','7D963B',.68),('Vein','647C30',.7),('Stitch','E2BD83',.85)]: c.material(*args)
c.sphere('Tunic',(0,.06,.55),(.47,.34,.49),'Tunic','Body')
for side in (-1,1):
    c.sphere('Boot',(side*.23,-.035,.12),(.18,.25,.115),'Leather','Root')
# Scalloped cream hem.
for i in range(32):
    a=i*math.tau/32
    c.sphere('Hem petal',(.42*math.cos(a),.06+.29*math.sin(a),.29),(.08,.050,.057),'Hem','Body',segments=20,rings=14)
face=c.sphere('Face',(0,0,1.57),(.73,.49,.65),'Face',segments=96,rings=64)
for v in face.data.vertices:
    t=(v.co.z-1.57)/.65
    v.co.x*=1+.06*math.exp(-((t+.3)/.4)**2)
c.blush(face,'Face',[(-.48,1.37),(.48,1.37)],(.18,.115))
# Hair peeks out below the cap and around both cheeks.
for side in (-1,1):
    for j in range(4):
        x=side*(.54+j*.04); z=1.75-j*.16
        c.leaf('Side hair',[(x,.025,z+.16),(x+side*.08,-.16,z),(x+side*.01,-.21,z-.16),(x+side*.09,-.18,z-.20)],.045,'Hair',depth=.033)
for i in range(7):
    x=-.34+i*.095
    c.leaf('Fringe',[(x+.13,-.47,2.14),(x+.13,-.50,2.06),(x+.05,-.50,2.04),(x-.025,-.475,1.995+.035*math.sin(i*1.7))],.026,'Hair',depth=.012)
c.face(.345,1.60,-.463,1.305,-.474,.14,open_mouth=False)
c.sphere('Button nose',(0,-.507,1.46),(.139,.112,.119),'Nose',segments=48,rings=32)
# A soft dome overlaps the folded tail behind the left cheek.
hat=c.sphere('Cap crown',(-.13,.14,2.12),(.80,.57,.66),'Hat',segments=80,rings=56)
for v in hat.data.vertices:
    if v.co.z<1.84: v.co.z=1.84
    # Leave room for the arched opening around the face.
    if v.co.y<0:
        opening=1.81+.40*math.sqrt(max(0,1-(v.co.x/.80)**2))
        v.co.z=max(v.co.z,opening)
    v.co.x+=.018*math.sin(v.co.z*19+v.co.y*3)
    v.co.y+=.012*math.sin(v.co.z*24+v.co.x*5)
c.sphere('Folded cap back',(-.63,.22,2.14),(.29,.38,.40),'Hat',segments=48,rings=32)
c.sphere('Cap droop',(-.79,.15,1.83),(.20,.26,.28),'Hat',segments=48,rings=32)
c.sphere('Cap tip',(-.83,.07,1.63),(.14,.18,.17),'Hat')
# Merge the overlapping cloth volumes into one soft cap.
import bpy
cap_parts=[o for o in c.parts if o.name in {'Cap crown','Folded cap back','Cap droop','Cap tip'}]
c.parts=[o for o in c.parts if o not in cap_parts]
cap=c.join(cap_parts,'Cloth cap')
remesh=cap.modifiers.new('Joined cloth','REMESH'); remesh.mode='VOXEL'; remesh.voxel_size=.024; remesh.use_smooth_shade=True
bpy.ops.object.modifier_apply(modifier=remesh.name)
smooth=cap.modifiers.new('Soft folds','SMOOTH'); smooth.factor=1; smooth.iterations=5
bpy.ops.object.modifier_apply(modifier=smooth.name)
cap.vertex_groups.clear(); group=cap.vertex_groups.new(name='Head'); group.add(list(range(len(cap.data.vertices))),1,'REPLACE')
uv=cap.data.uv_layers.new(name='UVMap')
for polygon in cap.data.polygons:
    for index in polygon.loop_indices:
        v=cap.data.vertices[cap.data.loops[index].vertex_index].co
        uv.data[index].uv=(math.atan2(v.y,v.x)/math.tau+.5,v.z/3)
for v in cap.data.vertices:
    theta=math.atan2(v.co.y-.14,v.co.x+.13)
    latitude=math.asin(max(-1,min(1,(v.co.z-2.12)/.66)))
    amount=.016*math.sin(latitude*13+theta*1.7)*math.exp(-((theta+2.3)/.75)**2)
    normal=Vector(((v.co.x+.13)/.80,(v.co.y-.14)/.57,(v.co.z-2.12)/.66)).normalized()
    v.co+=normal*amount
c.parts.append(cap)
# The thick rolled brim arches above the face.
points=[]
for i in range(129):
    a=i*math.tau/128
    points.append((.735*math.cos(a),.035+.50*math.sin(a),1.81+.40*max(0,-math.sin(a))+.035*math.cos(a)))
c.tube('Rolled cap brim',points,.087,'HatEdge',cyclic=True)
c.sphere('Cap end',(-.86,-.005,1.61),(.115,.13,.12),'Hat')
c.tube('Cap tie',[(-.86,-.02,1.58),(-.86,-.025,1.46),(-.80,-.035,1.38)],.028,'HatEdge')
c.sphere('Cap button',(-.80,-.035,1.35),(.077,.065,.09),'Hat')
c.leaf('Hat leaf left',[(.46,.08,2.38),(.45,.08,2.57),(.45,.08,2.69),(.50,.08,2.76)],.077,'Leaves',depth=.019)
c.leaf('Hat leaf right',[(.46,.09,2.41),(.65,.09,2.45),(.69,.09,2.61),(.71,.09,2.66)],.082,'Leaves',depth=.021)
# Resting hands hold the vest edges below the face.
for side in (-1,1):
    c.sphere('Sleeve',(side*.39,-.025,.78),(.17,.22,.20),'Tunic','Body')
    c.sphere('Cuff',(side*.36,-.21,.78),(.12,.09,.135),'Hem','Body')
    c.sphere('Hand',(side*.30,-.305,.83),(.115,.10,.14),'Skin','Body')
    c.sphere('Thumb',(side*.22,-.355,.79),(.066,.047,.088),'Skin','Body',segments=24,rings=16)
    for j in range(3):
        x=side*(.27+.042*j)
        c.tube('Finger crease',[(x,-.400,.835),(x+side*.009,-.398,.80)],.0035,'Hem','Body')
for side in (-1,1):
    c.leaf('Vest lapel',[(side*.20,-.25,.94),(side*.15,-.32,.85),(side*.10,-.34,.73),(side*.16,-.34,.65)],.08,'LeatherLight','Body',depth=.018)
for i in range(5):
    x=-.30+i*.15
    c.leaf('Tunic fold',[(x,-.23,.79),(x-.025,-.29,.63),(x+.025,-.30,.48),(x,-.25,.37+.025*math.cos(i))],.085,'Tunic','Body',depth=.014)
# Belt with a round clasp.
c.tube('Belt',[(.46*math.cos(i*math.tau/96),.06+.327*math.sin(i*math.tau/96),.48) for i in range(96)],.044,'Leather','Body',cyclic=True)
c.sphere('Buckle',(.06,-.294,.48),(.070,.026,.062),'LeatherLight','Body')
c.sphere('Buckle inset',(.06,-.319,.48),(.026,.007,.025),'Leather','Body')
bag=c.optional.setdefault('satchel',[])
c.tube('Satchel strap',[(.26-.73*t,.02-.33*math.sin(math.pi*t),.97-.44*t) for t in [i/48 for i in range(49)]],.024,'Leather','Body',bag)
c.sphere('Acorn bag',(-.49,-.17,.43),(.19,.15,.23),'Leather','Body',bag)
c.sphere('Acorn cap',(-.49,-.19,.59),(.20,.15,.091),'LeatherLight','Body',bag)
for j in range(9):
    a=math.pi+j*math.pi/8
    c.tube('Acorn seam',[(-.49+.15*math.cos(a),-.32,.57),(-.49+.13*math.cos(a),-.327,.61)],.004,'Stitch','Body',bag)
c.sphere('Bag clasp',(-.48,-.34,.54),(.030,.015,.035),'LeatherLight','Body',bag)
for obj in c.parts+c.eyes+c.mouth+[o for group in c.optional.values() for o in group]:
    bone=obj.vertex_groups[0].name
    for v in obj.data.vertices:
        if bone in ('Body','Root'):
            if obj.name.startswith(('Hand','Thumb','Finger crease')):
                v.co.z=.83*.68+(v.co.z-.83)
            else:
                v.co.z*=.68
            v.co.x*=.92
        else:
            v.co.z-=.30
c.eye_z-=.30
c.mouth_z-=.30
c.finish(.66,2.53,1.25)
