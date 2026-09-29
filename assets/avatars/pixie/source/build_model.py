"""Build the Pixie source scene. Run in Blender's Python console.

Set PIXIE_DIR to the asset folder before execution. A new scene keeps any
open work intact. The source meshes use meters, with the face toward -Y.
"""

import bpy
import math
from mathutils import Vector
from mathutils.noise import noise_vector
import bmesh
from pathlib import Path

ROOT = Path(PIXIE_DIR)
if bpy.data.collections.get('Pixie_Character'):
    raise RuntimeError('Start a new Blender file before rebuilding the Pixie source scene.')
scene = bpy.data.scenes.new('Pixie_Studio')
bpy.context.window.scene = scene
scene.render.engine = 'CYCLES'
scene.cycles.samples = 48
scene.cycles.use_denoising = True
scene.render.resolution_x = 900
scene.render.resolution_y = 1000
scene.render.resolution_percentage = 100
scene.render.image_settings.file_format = 'PNG'
scene.render.image_settings.color_mode = 'RGBA'
scene.render.film_transparent = True
scene.render.fps = 30
scene.view_settings.view_transform = 'AgX'
scene.view_settings.exposure = 0.0
scene.view_settings.look = 'AgX - Medium High Contrast'
scene.world = bpy.data.worlds.new('Pixie_World')
scene.world.use_nodes = True
scene.world.node_tree.nodes['Background'].inputs['Color'].default_value = (0.72, 0.77, 0.64, 1)
scene.world.node_tree.nodes['Background'].inputs['Strength'].default_value = 0.25

character = bpy.data.collections.new('Pixie_Character')
studio = bpy.data.collections.new('Pixie_Lighting')
scene.collection.children.link(character)
scene.collection.children.link(studio)


def linear(hex_value):
    rgb = [int(hex_value[i:i+2], 16) / 255 for i in (0, 2, 4)]
    return tuple(v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in rgb)


def material(name, color, roughness=0.55, metal=0):
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = (*linear(color), 1)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get('Principled BSDF')
    bsdf.inputs['Base Color'].default_value = mat.diffuse_color
    bsdf.inputs['Roughness'].default_value = roughness
    bsdf.inputs['Metallic'].default_value = metal
    return mat


mats = {
    'Skin': material('Skin', 'FFE0AB', 0.53),
    'EarInner': material('EarInner', 'EDA079', 0.62),
    'Body': material('Body', 'ADCA64', 0.78),
    'Leaves': material('Leaves', '789D36', 0.55),
    'LeafLight': material('LeafLight', '93B844', 0.58),
    'LeafDark': material('LeafDark', '627F2B', 0.6),
    'LeafVeins': material('LeafVeins', '799443', 0.62),
    'EyeRim': material('EyeRim', '9B713A', 0.30),
    'Eyes': material('Eyes', '21160B', 0.12),
    'EyeLight': material('EyeLight', 'FFFFFF', 0.22),
    'Mouth': material('Mouth', '66251C', 0.61),
    'Tongue': material('Tongue', 'F3988A', 0.66),
    'Leather': material('Leather', '98632D', 0.72),
    'LeatherLight': material('LeatherLight', 'B8853D', 0.6),
    'Stitch': material('Stitch', 'D6B574', 0.85),
    'Metal': material('Metal', 'BA8B41', 0.32, 0.65),
    'Scarf': material('Scarf', 'CD715C', 0.88),
}

# This packed normal image gives Blender and glTF the same fine surface grain.
import numpy as np
size=512
rng=np.random.default_rng(41)
height=rng.random((size,size)).astype(np.float32)
height=(height+np.roll(height,1,0)+np.roll(height,1,1))/3
nx=(np.roll(height,-1,1)-np.roll(height,1,1))*.48
ny=(np.roll(height,-1,0)-np.roll(height,1,0))*.48
nz=np.ones_like(nx)
norm=np.sqrt(nx*nx+ny*ny+nz*nz)
pixels=np.stack((nx/norm*.5+.5,ny/norm*.5+.5,nz/norm*.5+.5,np.ones_like(nx)),axis=-1)
texture=bpy.data.images.new('Pixie surface grain',width=size,height=size,alpha=True)
texture.colorspace_settings.name='Non-Color'
texture.pixels.foreach_set(pixels.ravel())
(ROOT/'textures').mkdir(exist_ok=True)
texture.filepath_raw=str(ROOT/'textures'/'surface-normal.png'); texture.file_format='PNG'; texture.save(); texture.pack()

def surface_grain(mat,strength):
    nodes=mat.node_tree.nodes; links=mat.node_tree.links
    image=nodes.new('ShaderNodeTexImage'); image.image=texture; image.label='Fine clay grain'
    normal=nodes.new('ShaderNodeNormalMap'); normal.inputs['Strength'].default_value=strength
    links.new(image.outputs['Color'],normal.inputs['Color'])
    links.new(normal.outputs['Normal'],nodes['Principled BSDF'].inputs['Normal'])

for name,strength in [('Body',.65),('Leaves',.47),('LeafLight',.47),('LeafDark',.47),('Leather',.75),('LeatherLight',.75),('Scarf',.85)]:
    surface_grain(mats[name],strength)

parts = []
eye_parts = []
mouth_parts = []
accessories = {'Glasses': [], 'Scarf': [], 'Satchel': []}


def place(obj, name, mat, bone='Head', group=None):
    obj.name = name
    for old in list(obj.users_collection):
        old.objects.unlink(obj)
    character.objects.link(obj)
    if mat:
        obj.data.materials.append(mats[mat])
    if obj.type == 'MESH':
        for polygon in obj.data.polygons:
            polygon.use_smooth = True
        vg = obj.vertex_groups.new(name=bone)
        vg.add(list(range(len(obj.data.vertices))), 1, 'REPLACE')
    (parts if group is None else group).append(obj)
    return obj


def select(obj):
    bpy.ops.object.select_all(action='DESELECT')
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj


def sphere(name, location, scale, mat, bone='Head', group=None, segments=32, rings=20):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=segments, ring_count=rings, location=location)
    obj = bpy.context.object
    obj.scale = scale
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    return place(obj, name, mat, bone, group)


def mesh(name, vertices, faces, mat, bone='Head', group=None):
    data = bpy.data.meshes.new(name)
    data.from_pydata(vertices, [], faces)
    data.update()
    obj = bpy.data.objects.new(name, data)
    return place(obj, name, mat, bone, group)


def tube(name, points, radius, mat, bone='Head', group=None, cyclic=False):
    data = bpy.data.curves.new(name, 'CURVE')
    data.dimensions = '3D'
    data.resolution_u = 8
    data.bevel_depth = radius
    data.bevel_resolution = 2
    spline = data.splines.new('POLY')
    spline.points.add(len(points)-1)
    for p, coordinate in zip(spline.points, points):
        p.co = (*coordinate, 1)
    spline.use_cyclic_u = cyclic
    obj = bpy.data.objects.new(name, data)
    character.objects.link(obj)
    select(obj)
    bpy.ops.object.convert(target='MESH')
    return place(bpy.context.object, name, mat, bone, group)


def bezier(a, b, c, d, t):
    return (1-t)**3*a + 3*(1-t)**2*t*b + 3*(1-t)*t*t*c + t**3*d


def leaf(name, controls, width, mat='Leaves', bone='Head', normal=(0, -1, 0), veins=True, depth=.055):
    """A closed leaf with a soft ridge, curved edges, and a turned tip."""
    controls = [Vector(p) for p in controls]
    normal = Vector(normal).normalized()
    steps, around = 32, 20
    vertices, faces, centers = [], [], []
    for i in range(steps+1):
        t = i / steps
        center = bezier(*controls, t)
        tangent = (bezier(*controls, min(1,t+.002))-bezier(*controls, max(0,t-.002))).normalized()
        cross = tangent.cross(normal).normalized()
        bulge = max(.0008, math.sin(math.pi*t)) ** .66
        breadth = width * bulge * (1.18-.36*t)
        centers.append((center, cross, breadth, bulge))
        for j in range(around):
            a = j/around*math.tau
            u = math.cos(a)
            front = math.sin(a)
            thickness = depth*bulge*front
            if front > 0:
                thickness += .018 * bulge * (1-abs(u))
            edge_wave = 1+.025*math.sin(t*math.pi*7+u*2)*abs(u)**3
            point = center + cross*u*breadth*edge_wave + normal*thickness
            vertices.append(tuple(point))
    for i in range(steps):
        for j in range(around):
            k=i*around+j; n=i*around+(j+1)%around
            faces.append((k,k+around,n+around,n))
    faces += [tuple(reversed(range(around))), tuple(steps*around+j for j in range(around))]
    obj=mesh(name,vertices,faces,mat,bone)
    bm=bmesh.new(); bm.from_mesh(obj.data); bmesh.ops.recalc_face_normals(bm,faces=bm.faces); bm.to_mesh(obj.data); bm.free()
    uv=obj.data.uv_layers.new(name='UVMap')
    for polygon in obj.data.polygons:
        for loop_index in polygon.loop_indices:
            vertex_index=obj.data.loops[loop_index].vertex_index
            uv.data[loop_index].uv=((vertex_index%around)/around,(vertex_index//around)/steps)
    if veins:
        # Fine veins follow the leaf surface. They remain soft at avatar size.
        line=[]
        for center,cross,breadth,bulge in centers[2:-2]:
            line.append(tuple(center+normal*((depth+.018)*bulge+.0005)))
        tube(name+'_midrib',line,.0013,'LeafVeins',bone)
        for index in (9,15,21,26):
            for sign in (-1,1):
                branch=[]
                for k in range(10):
                    u=k/9*.76
                    t=(index+k/9*4)/steps
                    center=bezier(*controls,t)
                    tangent=(bezier(*controls,t+.002)-bezier(*controls,t-.002)).normalized()
                    cross=tangent.cross(normal).normalized()
                    bulge=math.sin(math.pi*t)**.66
                    breadth=width*bulge*(1.18-.36*t)
                    height=depth*bulge*math.sqrt(1-u*u)+.018*bulge*(1-u)
                    branch.append(tuple(center+cross*u*breadth*sign+normal*(height+.001)))
                tube(name+'_vein',branch,.0008,'LeafVeins',bone)
    return obj


# The head is broad at the cheeks. The small body sits behind it.
body=sphere('BodyShape',(0,.16,.64),(.51,.375,.61),'Body','Body',segments=48,rings=32)
for v in body.data.vertices:
    t=(v.co.z-.03)/1.22
    v.co.x*=1.12-.22*t
head=sphere('HeadShape',(0,0,1.79),(.91,.59,.70),'Skin',segments=96,rings=64)
for v in head.data.vertices:
    t=(v.co.z-1.79)/.70
    v.co.x*=1+.085*math.exp(-((t+.32)/.43)**2)
    # A full lower cheek and a gentle chin replace the egg-shaped jaw.
    if t<-.25: v.co.z+=.045*((-t-.25)/.75)**2

for side,suffix in [(-1,'L'),(1,'R')]:
    sphere('Foot.'+suffix,(side*.235,-.04,.13),(.175,.245,.15),'Body','Foot.'+suffix)
    # The ear has a broad triangular root, a rolled edge, and a fine point.
    border=[]
    curves=[((.78,1.96),(1.15,1.925),(1.55,2.045)),((1.55,2.045),(1.31,1.745),(.85,1.60)),((.85,1.60),(.72,1.68),(.78,1.96))]
    for a,b,c in curves:
        for i in range(20):
            t=i/20
            border.append(((1-t)**2*a[0]+2*(1-t)*t*b[0]+t*t*c[0],(1-t)**2*a[1]+2*(1-t)*t*b[1]+t*t*c[1]))
    vertices=[]; faces=[]; colors=[]
    for layer in (0,1):
        for ring in range(13):
            r=max(.0001,ring/12)
            for x,z in border:
                px=.93+(x-.93)*r; pz=1.79+(z-1.79)*r
                y=.02-.082*math.sin(math.pi*r)**.7 if layer==0 else .055+.035*math.sin(math.pi*r)
                vertices.append((side*px,y,pz))
                mix=max(0,1-(r/.84)**5)*.85 if layer==0 else 0
                colors.append(tuple((1-mix)*a+mix*b for a,b in zip(linear('FFE0AB'),linear('EC997D')))+(1,))
    n=len(border); offset=13*n
    for layer in (0,1):
        for ring in range(12):
            for j in range(n):
                k=layer*offset+ring*n+j; nxt=layer*offset+ring*n+(j+1)%n
                faces.append((k,nxt,nxt+n,k+n))
    for j in range(n):
        k=12*n+j; nxt=12*n+(j+1)%n
        faces.append((k,k+offset,nxt+offset,nxt))
    ear=mesh('Ear.'+suffix,vertices,faces,'Skin','Ear.'+suffix)
    bm=bmesh.new(); bm.from_mesh(ear.data); bmesh.ops.recalc_face_normals(bm,faces=bm.faces); bm.to_mesh(ear.data); bm.free()
    ear_mat=material('EarSkin.'+suffix,'FFFFFF',.6)
    cnode=ear_mat.node_tree.nodes.new('ShaderNodeVertexColor'); cnode.layer_name='Color'
    ear_mat.node_tree.links.new(cnode.outputs['Color'],ear_mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'])
    ear.data.materials.clear(); ear.data.materials.append(ear_mat)
    attr=ear.data.color_attributes.new(name='Color',type='FLOAT_COLOR',domain='POINT')
    for datum,value in zip(attr.data,colors): datum.color=value

# A thin warm iris edge surrounds each dark, near-round eye.
for x in (-.36,.36):
    sphere('EyeRim',(x,-.552,1.779),(.142,.043,.156),'EyeRim',group=eye_parts,segments=40,rings=28)
    sphere('Eye',(x,-.574,1.779),(.137,.048,.149),'Eyes',group=eye_parts,segments=48,rings=32)
    sphere('Catchlight',(x-.036,-.618,1.836),(.034,.012,.035),'EyeLight',group=eye_parts,segments=24,rings=16)
    sphere('CatchlightSmall',(x+.046,-.617,1.729),(.009,.006,.010),'EyeLight',group=eye_parts,segments=16,rings=10)
    tube('Brow',[(x-.057+i*.0095,-.479,2.095+.023*math.sin(i/12*math.pi)) for i in range(13)],.012,'LeafDark')
sphere('Nose',(0,-.585,1.627),(.039,.023,.027),'Skin',segments=24,rings=16)

# The upper edge dips in the middle, so both corners lift into a smile.
verts=[(0,-.571,1.527)]
for i in range(64):
    a=i/64*math.tau; u=math.cos(a); sy=math.sin(a)
    z=1.565-.027*(1-u*u) if sy>=0 else 1.565-.114*(-sy)
    verts.append((.112*u,-.572,z))
mesh('SmileInside',verts,[(0,i+1,(i+1)%64+1) for i in range(64)],'Mouth',group=mouth_parts)
sphere('Tongue',(0,-.581,1.479),(.062,.007,.027),'Tongue',group=mouth_parts,segments=32,rings=16)

# Broad blush is painted into the skin and stays in the exported model.
face_material=material('FaceSkin','FFFFFF',.56)
color_node=face_material.node_tree.nodes.new('ShaderNodeVertexColor'); color_node.layer_name='Color'
face_material.node_tree.links.new(color_node.outputs['Color'],face_material.node_tree.nodes['Principled BSDF'].inputs['Base Color'])
head.data.materials.clear(); head.data.materials.append(face_material)
color=head.data.color_attributes.new(name='Color',type='FLOAT_COLOR',domain='POINT')
for v,datum in zip(head.data.vertices,color.data):
    x,y,z=v.co
    d=((abs(x)-.58)/.195)**2+((z-1.537)/.105)**2
    blush=math.exp(-d*.85)*.79 if y<-.12 else 0
    base=linear('FFE0AB'); pink=linear('F1838B')
    grain=1+.008*noise_vector(v.co*230)[0]
    datum.color=tuple(min(1,max(0,((1-blush)*a+blush*b)*grain)) for a,b in zip(base,pink))+(1,)

# Leaves overlap like a swept fringe. The crown never exposes a bald seam.
sphere('HairCap',(0,.12,1.98),(.93,.54,.64),'LeafDark',segments=48,rings=32)
for j in range(13):
    a=math.pi*j/12
    x=.83*math.cos(a); y=.10+.45*math.sin(a)
    leaf('BackLeaf.%02d'%j,[(x*.15,.13,2.51),(x*.91,y,2.58),(x*1.18,y+.06,1.69),(x*1.08,y,1.33+.08*math.sin(a))],.24,'Leaves' if j%3 else 'LeafLight',normal=(x*.8,.85,0),depth=.067)
# Back-to-front order keeps the broad leaf shoulders under the next layer.
bangs=[
    ([(.19,.015,2.56),(-.62,-.12,2.71),(-.78,-.39,2.03),(-1.19,-.25,2.04)],.285,'Leaves'),
    ([(.17,-.055,2.60),(-.32,-.39,2.60),(-.78,-.50,2.07),(-.87,-.37,1.78)],.282,'LeafLight'),
    ([(.18,-.12,2.58),(-.14,-.56,2.64),(-.48,-.61,2.15),(-.61,-.54,1.94)],.258,'Leaves'),
    ([(.22,-.13,2.59),(.02,-.58,2.53),(-.04,-.64,2.24),(-.29,-.61,2.075)],.229,'LeafLight'),
    ([(.20,-.045,2.565),(.48,-.31,2.50),(.70,-.44,2.35),(.98,-.29,2.40)],.183,'Leaves'),
    ([(.35,.015,2.49),(.68,-.24,2.40),(.83,-.37,2.12),(.89,-.23,1.91)],.174,'LeafLight'),
]
for i,(controls,width,mat) in enumerate(bangs): leaf('Fringe.%02d'%i,controls,width,mat,depth=.075)
for sign in (-1,1):
    leaf('SideLock',[(sign*.78,.015,2.21),(sign*1.02,-.16,2.0),(sign*.86,-.13,1.50),(sign*.98,-.02,1.37)],.146,'Leaves',depth=.055)
    leaf('SideTip',[(sign*.78,.10,1.75),(sign*.93,.04,1.64),(sign*.77,-.02,1.34),(sign*.92,.015,1.23)],.122,'LeafLight',depth=.042)
tube('SproutStem',[(.19,.07,2.49),(.21,.07,2.67),(.14,.07,2.79)],.017,'LeafVeins','Sprout')
leaf('SproutLeft',[(.17,.065,2.68),(-.08,.035,2.70),(-.32,.035,2.93),(-.38,.06,3.035)],.155,'Leaves','Sprout',depth=.023)
leaf('SproutRight',[(.17,.095,2.71),(.30,.10,2.78),(.36,.13,2.96),(.32,.15,3.055)],.094,'LeafLight','Sprout',depth=.018)

# A separate satchel can be removed without a second body model.
satchel = accessories['Satchel']
# A flat strap follows the same pear surface as the body.
def body_surface(x,z,front=True):
    t=(z-.64)/.61
    width=.51*(1.12-.22*(z-.03)/1.22)
    depth=.375*math.sqrt(max(.001,1-t*t-(x/width)**2))
    return .16-depth-.006 if front else .16+depth+.006

for front in (True,False):
    verts=[]; faces=[]
    for i in range(49):
        t=i/48
        x=.22-.54*t; z=1.17-.61*t
        for sign in (-1,1):
            px=x+sign*.021; pz=z-sign*.019
            verts.append((px,body_surface(px,pz,front),pz))
        if i<48:
            k=i*2; faces.append((k,k+1,k+3,k+2))
    strap=mesh('ShoulderStrap'+('Front' if front else 'Back'),verts,faces,'Leather','Body',satchel)
    select(strap)
    mod=strap.modifiers.new('Leather thickness','SOLIDIFY'); mod.thickness=.009
    bpy.ops.object.modifier_apply(modifier=mod.name)
    mod=strap.modifiers.new('Rounded strap edges','BEVEL'); mod.width=.005; mod.segments=3
    bpy.ops.object.modifier_apply(modifier=mod.name)
sphere('Bag',(-.34,-.305,.46),(.188,.139,.216),'Leather','Body',satchel)
sphere('BagFlap',(-.34,-.423,.57),(.187,.045,.098),'LeatherLight','Body',satchel)
sphere('BagButton',(-.33,-.472,.534),(.026,.012,.026),'Metal','Body',satchel,segments=20,rings=12)
for i in range(13):
    angle=math.pi+i/12*math.pi
    x=-.34+.151*math.cos(angle); z=.575+.063*math.sin(angle)
    tube('BagStitch',[(x-.007,-.464,z),(x+.007,-.464,z+.006)],.0035,'Stitch','Body',satchel)

glasses=accessories['Glasses']
for x in (-.36,.36):
    tube('GlassesRim',[(x+.224*math.cos(i/64*math.tau),-.66,1.784+.237*math.sin(i/64*math.tau)) for i in range(64)],.014,'Metal','Head',glasses,cyclic=True)
tube('GlassesBridge',[(-.137,-.66,1.80),(-.05,-.681,1.836),(.05,-.681,1.836),(.137,-.66,1.80)],.013,'Metal','Head',glasses)
for s in (-1,1):
    tube('GlassesTemple',[(s*.583,-.66,1.82),(s*.81,-.45,1.835),(s*.88,-.03,1.85)],.012,'Metal','Head',glasses)

scarf=accessories['Scarf']
tube('ScarfWrap',[(.35*math.cos(i/56*math.tau),.05+.31*math.sin(i/56*math.tau),1.092+.025*math.cos(i/56*math.tau)) for i in range(56)],.105,'Scarf','Body',scarf,cyclic=True)
sphere('ScarfKnot',(.26,-.30,1.04),(.139,.103,.128),'Scarf','Body',scarf)
for shift in (0,.13):
    obj=mesh('ScarfTail',[(.22+shift,-.32,1.01),(.35+shift,-.31,.99),(.43+shift,-.39,.54),(.29+shift,-.43,.51)],[(0,1,2,3)],'Scarf','Body',scarf)
    select(obj)
    mod=obj.modifiers.new('Cloth thickness','SOLIDIFY'); mod.thickness=.045
    bpy.ops.object.modifier_apply(modifier=mod.name)
    mod=obj.modifiers.new('Soft cloth edge','BEVEL'); mod.width=.035; mod.segments=3
    bpy.ops.object.modifier_apply(modifier=mod.name)
    for f in range(4):
        x=.30+shift+f*.029
        tube('ScarfFringe',[(x,-.42,.53),(x+.02,-.435,.45)],.012,'Scarf','Body',scarf)


def join(objects,name):
    bpy.ops.object.select_all(action='DESELECT')
    for obj in objects: obj.select_set(True)
    bpy.context.view_layer.objects.active=objects[0]
    bpy.ops.object.join()
    obj=bpy.context.object
    obj.name=name
    obj.data.name=name
    bm=bmesh.new(); bm.from_mesh(obj.data)
    bmesh.ops.triangulate(bm,faces=list(bm.faces))
    bm.to_mesh(obj.data); bm.free()
    return obj


def chibi_z(z):
    return z*.68 if z<1.12 else z-.3584


def head_z(z):
    return chibi_z(1.12)+(z-1.12)*1.08-.16


for obj in parts+eye_parts+mouth_parts+[o for group in accessories.values() for o in group]:
    bone=obj.vertex_groups[0].name
    for vertex in obj.data.vertices:
        x,y,z=vertex.co
        vertex.co.z=(chibi_z(.46)+(z-.46)*.95) if obj in accessories['Satchel'] and not obj.name.startswith('ShoulderStrap') else chibi_z(z)
        if obj in accessories['Scarf']:
            vertex.co.z=.61+(vertex.co.z-.69)*.85
        if bone in ('Head','Sprout') or bone.startswith('Ear.'):
            vertex.co.x=x*.9
            vertex.co.z=head_z(z)
        if obj in eye_parts:
            sign=-1 if x<0 else 1
            vertex.co.x=x-sign*.036
            vertex.co.z=head_z(1.779)+(z-1.779)

exec(compile((ROOT/'source/hands.py').read_text(),'hands.py','exec'))
parts.extend(make_resting_hands(character,mats['Skin'],mats['Body']))
base=join(parts,'Pixie')
eyes=join(eye_parts,'FaceEyes')
mouth=join(mouth_parts,'FaceMouth')
for obj in (eyes,mouth): obj.shape_key_add(name='Basis',from_mix=False)
blink=eyes.shape_key_add(name='Blink',from_mix=False)
blink.value=0
for point in blink.data:
    point.co.z=head_z(1.779)+(point.co.z-head_z(1.779))*.07
    point.co.y+=.035
for name,sx,sz,dz in [('Smile',1.22,1.16,0),('Surprise',.56,1.10,.01),('Concern',1,.12,-.015)]:
    key=mouth.shape_key_add(name=name,from_mix=False)
    key.value=0
    for point in key.data:
        point.co.x*=sx
        point.co.z=head_z(1.54)+(point.co.z-head_z(1.54))*sz+dz
        if name=='Concern': point.co.z-=.22*abs(point.co.x)
optional=[]
for name,objects in accessories.items():
    obj=join(objects,'Accessory_'+name)
    obj['accessory']=name.lower()
    obj['default_visible']=name=='Satchel'
    obj.hide_render=name!='Satchel'
    obj.hide_set(name!='Satchel')
    optional.append(obj)

# One small skeleton carries the entire character and its attachment points.
armature=bpy.data.armatures.new('Pixie_Rig')
rig=bpy.data.objects.new('Pixie_Rig',armature)
character.objects.link(rig)
select(rig)
bpy.ops.object.mode_set(mode='EDIT')
bones={
    'Root':((0,0,0),(0,0,.3),None),
    'Body':((0,0,.30),(0,0,1.08),'Root'),
    'Head':((0,0,1.12),(0,0,2.35),'Body'),
    'Sprout':((.19,.07,2.51),(.17,.08,2.97),'Head'),
}
for s,suffix in [(-1,'L'),(1,'R')]:
    bones['Ear.'+suffix]=((s*.77,0,1.68),(s*1.48,0,2.035),'Head')
    bones['Foot.'+suffix]=((s*.235,0,.30),(s*.235,-.08,.08),'Root')
for name,(head,tail,parent) in bones.items():
    bone=armature.edit_bones.new(name)
    is_head=name in ('Head','Sprout') or name.startswith('Ear.')
    bone.head=(head[0]*(.9 if is_head else 1),head[1],head_z(head[2]) if is_head else chibi_z(head[2]))
    bone.tail=(tail[0]*(.9 if is_head else 1),tail[1],head_z(tail[2]) if is_head else chibi_z(tail[2]))
    if parent: bone.parent=armature.edit_bones[parent]
bpy.ops.object.mode_set(mode='OBJECT')
rig.show_in_front=True
armature.display_type='STICK'
for obj in [base,eyes,mouth]+optional:
    obj.parent=rig
    modifier=obj.modifiers.new('Pixie skin','ARMATURE')
    modifier.object=rig
for name,bone,position in [('Head','Head',(.18,0,2.55)),('Face','Head',(0,-.65,1.78)),('Back','Body',(0,.38,.90))]:
    socket=bpy.data.objects.new('Socket_'+name,None)
    character.objects.link(socket)
    socket.empty_display_type='PLAIN_AXES'; socket.empty_display_size=.10
    socket.parent=rig; socket.parent_type='BONE'; socket.parent_bone=bone
    bpy.context.view_layer.update()
    socket.matrix_world.translation=(position[0]*(.9 if bone=='Head' else 1),position[1],head_z(position[2]) if bone=='Head' else chibi_z(position[2]))
    socket['attachment']=name.lower()
rig['family']='pixie'
rig['front']='-Y in Blender; +Z in glTF'
rig['reference']='source/reference.png'
rig['expressions']='Blink, Smile, Surprise, Concern'


def aim(obj,at):
    obj.rotation_euler=(Vector(at)-obj.location).to_track_quat('-Z','Y').to_euler()


camera_data=bpy.data.cameras.new('Portrait')
camera=bpy.data.objects.new('Portrait',camera_data)
studio.objects.link(camera)
camera.location=(3.0,-10,2.85)
aim(camera,(-.06,0,1.34))
camera_data.type='ORTHO'; camera_data.ortho_scale=3.4
scene.camera=camera
for name,location,power,size,color in [
    ('Key',(-3,-4,6),520,4.0,(1,.91,.76)),
    ('Fill',(4,-2,3),120,3.0,(.88,.93,1)),
    ('Bounce',(0,-3,.5),65,4.0,(1,.92,.79)),
    ('Rim',(1,3,5),460,3.0,(1,.92,.72)),
]:
    data=bpy.data.lights.new(name,'AREA'); data.energy=power; data.shape='DISK'; data.size=size; data.color=color
    obj=bpy.data.objects.new(name,data); studio.objects.link(obj); obj.location=location; aim(obj,(0,0,1.7))
    if name=='Bounce':
        data.specular_factor=0
        obj.visible_glossy=False

# The ground is used only for the optional studio portrait.
bpy.ops.mesh.primitive_plane_add(size=200,location=(0,0,-.023))
ground=bpy.context.object; ground.name='Portrait ground'
for collection in list(ground.users_collection): collection.objects.unlink(ground)
studio.objects.link(ground)
ground.data.materials.append(material('Studio floor','FFF3D7',.82))
ground.is_shadow_catcher=False
ground.hide_render=True

select(rig)
for area in bpy.context.screen.areas:
    if area.type=='VIEW_3D':
        area.spaces.active.region_3d.view_perspective='CAMERA'
        area.spaces.active.overlay.show_overlays=False
        area.spaces.active.shading.type='MATERIAL'

# Relative to the saved file, so the file holds no machine path.
scene['asset_folder']='//'
scene['reference_note']='Pixie from the user-supplied Little Sprites illustration. Back and side views are interpreted.'
bpy.ops.wm.save_as_mainfile(filepath=str(ROOT/'pixie.blend'))
bpy.ops.file.make_paths_relative()
bpy.ops.wm.save_mainfile()
result={'scene':scene.name,'mesh_vertices':sum(len(o.data.vertices) for o in character.objects if o.type=='MESH'),'bones':len(armature.bones),'blend':str(ROOT/'pixie.blend')}
