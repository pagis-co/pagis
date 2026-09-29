"""Build small fixed hands in the final character coordinates."""

import bpy
import math
import bmesh
from mathutils import Matrix, Vector


def make_resting_hands(collection, skin_material, body_material):
    """Return two soft palms and short sleeves, all weighted to Body."""
    objects = []

    def select(obj):
        bpy.ops.object.select_all(action='DESELECT')
        obj.select_set(True)
        bpy.context.view_layer.objects.active = obj

    def ellipsoid(name, center, scale, angle=0):
        bpy.ops.mesh.primitive_uv_sphere_add(segments=24, ring_count=16)
        obj = bpy.context.object
        obj.name = name
        rotation = Matrix.Rotation(angle, 3, 'Y')
        for vertex in obj.data.vertices:
            vertex.co = rotation @ Vector(tuple(vertex.co[i]*scale[i] for i in range(3))) + Vector(center)
        for old in list(obj.users_collection):
            old.objects.unlink(obj)
        collection.objects.link(obj)
        return obj

    def finish(obj, material):
        obj.data.materials.clear()
        obj.data.materials.append(material)
        for polygon in obj.data.polygons:
            polygon.material_index = 0
            polygon.use_smooth = True
        group = obj.vertex_groups.new(name='Body')
        group.add(list(range(len(obj.data.vertices))), 1, 'REPLACE')
        bm = bmesh.new()
        bm.from_mesh(obj.data)
        bmesh.ops.triangulate(bm, faces=list(bm.faces))
        bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
        bm.to_mesh(obj.data)
        bm.free()
        objects.append(obj)

    for side, suffix in [(-1, 'L'), (1, 'R')]:
        pieces = [
            ellipsoid('Palm', (0, 0, -.025), (.100, .061, .095)),
            ellipsoid('Wrist', (-.005, .008, -.092), (.052, .048, .052)),
            ellipsoid('Thumb', (-.100, -.008, -.016), (.041, .045, .065), -.60),
        ]
        # Four short fingers share the palm. Their rounded tips stay distinct.
        fingers = [
            (-.072, .000, .062, .027, .056, -.15),
            (-.024, -.008, .079, .030, .065, -.04),
            (.027, -.002, .070, .029, .061, .08),
            (.074, .008, .048, .026, .053, .20),
        ]
        for index, (x, y, z, width, height, angle) in enumerate(fingers):
            pieces.append(ellipsoid('Finger'+str(index), (x, y, z), (width, .041, height), angle))
        select(pieces[0])
        for part in pieces: part.select_set(True)
        bpy.ops.object.join()
        hand = bpy.context.object
        hand.name = 'RestingPalm.'+suffix
        remesh = hand.modifiers.new('Join palm and fingers', 'REMESH')
        remesh.mode = 'VOXEL'
        remesh.voxel_size = .006
        remesh.use_smooth_shade = True
        bpy.ops.object.modifier_apply(modifier=remesh.name)
        smooth = hand.modifiers.new('Soft finger roots', 'SMOOTH')
        smooth.factor = .65
        smooth.iterations = 5
        bpy.ops.object.modifier_apply(modifier=smooth.name)
        reduce = hand.modifiers.new('Compact hand mesh', 'DECIMATE')
        reduce.ratio = .30
        bpy.ops.object.modifier_apply(modifier=reduce.name)
        # Rotate each open palm outward. The thumb points toward the face.
        turn = Matrix.Rotation(side*.25, 3, 'Y')
        for vertex in hand.data.vertices:
            vertex.co.x *= side
            vertex.co = turn @ vertex.co + Vector((side*.60, -.335, .75))
        for layer in list(hand.data.uv_layers):
            hand.data.uv_layers.remove(layer)
        uv = hand.data.uv_layers.new(name='UVMap')
        for polygon in hand.data.polygons:
            for loop_index in polygon.loop_indices:
                vertex = hand.data.vertices[hand.data.loops[loop_index].vertex_index]
                uv.data[loop_index].uv = (vertex.co.x*side, vertex.co.z)
        finish(hand, skin_material)
        start = Vector((side*.38, .02, .54))
        end = Vector((side*.58, -.32, .66))
        axis = (end-start).normalized()
        across = axis.cross(Vector((0, 0, 1))).normalized()
        up = axis.cross(across).normalized()
        vertices, faces = [], []
        for ring in range(17):
            t = ring/16
            center = start.lerp(end, t)
            radius = max(.0005, math.sin(math.pi*t))**.45*(.137*(1-t)+.068*t)
            for index in range(24):
                angle = index/24*math.tau
                vertices.append(center+radius*(math.cos(angle)*across+math.sin(angle)*up))
        for ring in range(16):
            for index in range(24):
                a = ring*24+index
                b = ring*24+(index+1)%24
                faces.append((a, b, b+24, a+24))
        faces.extend([tuple(reversed(range(24))), tuple(16*24+i for i in range(24))])
        data = bpy.data.meshes.new('RestingSleeve.'+suffix)
        data.from_pydata(vertices, [], faces)
        data.update()
        uv = data.uv_layers.new(name='UVMap')
        for polygon in data.polygons:
            for loop_index in polygon.loop_indices:
                index = data.loops[loop_index].vertex_index
                uv.data[loop_index].uv = ((index%24)/24, (index//24)/16)
        sleeve = bpy.data.objects.new('RestingSleeve.'+suffix, data)
        collection.objects.link(sleeve)
        finish(sleeve, body_material)
    return objects
