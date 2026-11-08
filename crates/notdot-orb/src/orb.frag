      #ifdef GL_FRAGMENT_PRECISION_HIGH
      precision highp float;
      #else
      precision mediump float;
      #endif
      uniform vec2 resolution;
      uniform vec3 tint;
      uniform float phase;
      uniform float energy;
      uniform float listening;
      uniform float thinking;
      uniform float speaking;
      uniform float still;
      float band(float value, float width) { return exp(-value*value/width); }
      void main() {
        vec2 point = (gl_FragCoord.xy-resolution*.5)/min(resolution.x,resolution.y)*2.;

        float radius = .34*(1.+(1.-still)*energy*speaking*.075);
        vec2 position = point/radius;
        float radialDistance = length(position);
        float alpha = 0.;
        vec3 light = vec3(0.);
        float glow = exp(-radialDistance*radialDistance*1.55)*.17;
        light += tint*glow;
        alpha += glow*.5;
        float shellRadius = 1.15+listening*.13+(1.-still)*speaking*energy*.16;
        float shell = band(radialDistance-shellRadius,.000012);
        float angle = atan(position.y,position.x);
        float shellLight = .18+.24*pow(.5+.5*sin(angle+phase*.15),3.);
        light += mix(tint,vec3(1.),.4)*shell*(shellLight+speaking*(.35+energy*.45));
        alpha += shell*shellLight*.55;
        float arcAngle = mod(angle+phase*.7+6.2831853,6.2831853);
        float arc = band(radialDistance-1.35,.000035)*smoothstep(.2,1.2,arcAngle)*(1.-smoothstep(1.2,2.9,arcAngle))*thinking;
        light += mix(tint,vec3(1.),.45)*arc*.65;
        alpha += arc*.4;
        if (radialDistance < 1.) {
          float depth = sqrt(max(0.,1.-radialDistance*radialDistance));
          vec3 accumulated = vec3(0.);
          float density = 0.;
          for (int index=0; index<6; index++) {
            float slice = (float(index)+.5)/6.;
            float z = mix(-depth,depth,slice);
            vec3 samplePoint = vec3(position,z);
            float warp = .18*sin(samplePoint.x*3.3+z*2.2+phase*.42)
                       + .09*sin(samplePoint.y*4.1-z*3.1-phase*.31);
            float flow = samplePoint.y*.86 + samplePoint.x*.4 + warp
                       + .3*sin(samplePoint.x*1.8+z*1.9-phase*.26);
            float layer = band(flow-.12,.012) + .55*band(flow+.38,.004);
            float fine = pow(.5+.5*sin(flow*30.+z*2.2),18.)*.16;
            float brightness = (layer+fine)*(.42+.58*slice)*depth;
            vec3 shade = mix(tint,vec3(.93,.99,1.),clamp(layer*.25+slice*.10,0.,.55));
            accumulated += shade*brightness;
            density += brightness;
          }
          float waveLight = .42+speaking*(.18+energy*.65);
          light += accumulated*waveLight;
          alpha += min(density*.07,.32);
          float fresnel = pow(1.-depth,3.);
          vec3 normal = vec3(position,depth);
          float diffuse = max(0.,dot(normal,normalize(vec3(-.55,.7,1.))));
          light += tint*(.012+.03*diffuse);
          alpha += .08;
          float rim = band(radialDistance-.991,.000055);
          float edgeHighlight = .24+.65*pow(.5+.5*dot(normal.xy,normalize(vec2(-.7,.8))),3.);
          light += mix(tint,vec3(1.),.7)*(fresnel*.15+rim*edgeHighlight);
          alpha += fresnel*.2+rim*.38;
          float glint = exp(-dot(position-vec2(-.48,.58),position-vec2(-.48,.58))*16.)*.3;
          light += mix(tint,vec3(1.),.8)*glint;
          alpha += glint*.2;
          float edgeMask = (1.-smoothstep(.985,1.,radialDistance));
          light *= edgeMask;
          alpha *= edgeMask;
          // An almost opaque ink core supplies its own contrast, independent
          // of desktop pixels. Only the narrow perimeter becomes translucent.
          float coreAlpha = .995*(1.-smoothstep(.945,1.,radialDistance));
          vec3 ink = mix(vec3(.008,.014,.027),tint*.035,.35);
          vec3 colouredLight = light*mix(tint,vec3(1.),.2)*1.55;
          vec3 luminous = colouredLight/(vec3(1.)+colouredLight*.6);
          light = (ink+luminous*.82)*edgeMask;
          alpha = max(coreAlpha,alpha);

        }
        // Deterministic sparse particles outside the body.
        vec2 grid = floor(point*42.);
        vec2 local = fract(point*42.)-.5;
        float seed = fract(sin(dot(grid,vec2(127.1,311.7)))*43758.5453);
        float star = step(.991,seed)*exp(-dot(local,local)*260.)*step(1.38,radialDistance)*step(radialDistance,2.35);
        float twinkle = .55+.2*sin(phase*.7+seed*81.);
        light += mix(tint,vec3(1.),.45)*star*twinkle;
        alpha += star*twinkle;
        light *= 1.25;
        alpha = clamp(max(alpha,max(light.r,max(light.g,light.b))),0.,1.);
        gl_FragColor = vec4(min(light,vec3(alpha)),alpha);
      }
