package app;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public class Boxes {
    private final Map<String, List<Integer>> index = new HashMap<String, List<Integer>>();
    private List<String> names;

    public Boxes() {
        names = new ArrayList<String>();
    }

    public int total(List<String> words) {
        List<Integer> sizes = new ArrayList<Integer>(words.size());
        var copy = new ArrayList<String>(words);
        Comparator<String> byLength = new Comparator<String>() {
            @Override
            public int compare(String a, String b) {
                return Integer.compare(a.length(), b.length());
            }
        };
        copy.sort(byLength);
        int sum0;
        Integer boxed = new Integer(41);
        Long big = new Long(7L);
        Double ratio = new Double("0.5");
        Boolean flag = new Boolean(true);
        Character initial = new Character('x');
        Short small = new Short((short) 3);
        Byte tiny = new Byte((byte) 4);
        Float half = new Float(0.5f);
        sum0 = small + tiny + (int) (half * 2);
        int sum = sum0 + boxed + big.intValue() + (flag ? 1 : 0) + initial + (int) (ratio * 2);
        for (String w : copy) {
            sizes.add(new Integer(w.length()));
            sum += w.length();
        }
        return sum + sizes.size();
    }
}
