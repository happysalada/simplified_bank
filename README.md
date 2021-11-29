# Thoughts and consideration

I've tried to describe my thoughts after initially reading the brief to be more informative.
Those are relatively unstructered.

## After reading the brief

- Precision: the text mentions 4 decimal of precision. I'm assuming we are not talking about cent precision (which would be 6 digits then) but about currency precision (just 4 digits). Also do you round up or truncate ? I will assume truncate.
- Performance consideration: Do you allocate memory beforehand or do you do it as you go. Since there is no information about how many accounts there can potentially be, I'm going to do allocation as it comes. This can be optimized later.
- Performance consideration: sync vs async: Probably there will be optimisations in splitting the job into different threads (maybe a thread doing the IO and a thread recording transactions? this will need some testing to figure out of course). I will start with a naive synchronous implementation.
- Performance consideration: memory alignement: I won't consider the memory layout of the structs for now, but this can potentially be optimized.
- Performance consideration: since the brief mentions streaming data to memory, I will do that in the initial implementation.
- Reliability: Do you keep the system in memory or do you back it with the filesystem ? I'm going to start in memory to arrive at a solution faster, but I'll revisit this decision if I have time (probably using sqlite rather than postgres for simplicity). A database also has constraints to help define and respect rules of the system.
- cli: Will there be a trick with the number of arguments supplied to the cli ? It would have been nice to have something like, just use std::env::args for cli args, don't think about it.
- What about the disputed transactions ? The output of the program is the accounts, and you can indirectly see with the amounts held that there were disputes, but it would be nice to output another csv with transactions that are disputed.
- Validity of the data: like it says in the brief, I will ignore transactions that are not in the right format (not a well formatted string like "DePoSiT", not a u16 or u32)
- minor: a couple of typos
    garaunteed -> garanteed
    and should be reverse -> and should be reversed 
    resvole -> resolve
    missing "locked" in the following example
        client,available,held,total,  
        2,2,0,2,false 
        1,1.5,0,1.5,false

## After working out the implementation

- I started directly with tokio because of the "stream" requirement
- TransactionRecord should really be an enum with Deposit, Withdrawal... (TODO: implement a custom deserializer for TransactionRecord)
- total field inside Client should really be computed at serialization. (TODO: implement a custom serializer for Client)
- I didn't do any clean commit separation.
- I didn't take into account the case where a transaction is disputed twice.
- I didn't take into consideration disputing a resolve or a dispute.
- I think I misunderstood disputing a withdrawal, because for me, disputing a withdrawal does increase the total held.
- A dispute should be a struct with just a transaction id (if the client isn't part of the original transaction then the dispute should be ignored ?)
- My implementation doesn't work properly with disputes not having a trailing "," (and withdrawals)
- This should definitely have automated testing, I tested by hand for lack of time 
