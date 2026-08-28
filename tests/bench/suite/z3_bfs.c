/* z3_bfs — BREADTH-FIRST SEARCH over an adjacency list.
 * WHY: a graph walk is a queue of indices, an edge-list scan per node, and a
 * visited bitmap — three arrays with unrelated access patterns in one loop, and
 * the edge scan's trip count is the node's degree, so it is short and variable.
 * Nothing in the suite has an irregular inner trip count. */
#include <stdio.h>
#define NV 200000
#define DEG 6
static int adj[NV*DEG];
static int q[NV], dist[NV];
int main(void){
    long i, r; unsigned long s = 0, seed = 23u;
    for(i=0;i<NV*DEG;i++){ seed = seed*6364136223846793005UL + 1442695040888963407UL; adj[i] = (int)((seed>>33) % NV); }
    for(r=0;r<9;r++){
        long head = 0, tail = 0;
        for(i=0;i<NV;i++) dist[i] = -1;
        q[tail++] = (int)r; dist[r] = 0;
        while(head < tail){
            int u = q[head++]; int d = dist[u]; long k;
            for(k=0;k<DEG;k++){
                int v = adj[u*DEG+k];
                if(dist[v] < 0){ dist[v] = d + 1; q[tail++] = v; }
            }
        }
        for(i=0;i<NV;i+=997) s += (unsigned long)(dist[i] + 1);
    }
    printf("%lu\n", s);
    return 0;
}
